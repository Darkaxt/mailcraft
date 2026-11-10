# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Refresh the prepared fork draft without replacing its source-pinned input."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import urlencode

ROOT = Path(__file__).resolve().parents[2]


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def api(endpoint, *, method='GET', upload=None, pages=False):
    command = ['gh', 'api', '--hostname', 'github.com', '--method', method, endpoint]
    if pages:
        command.extend(['--paginate', '--slurp'])
    if upload is not None:
        command.extend(['--header', 'Content-Type: application/octet-stream',
                        '--header', 'Content-Length: ' + str(upload.stat().st_size), '--input', str(upload)])
    # Preserve the flushed numeric identity; never retry an uncertain mutation.
    output = subprocess.check_output(command, text=True, encoding='utf-8')
    return json.loads(output) if output.strip() else None


def assets(repository, release_id):
    pages = api(f'repos/{repository}/releases/{release_id}/assets?per_page=100', pages=True)
    return [item for page in pages for item in page]


def refresh(binding, repository, version, commit, directory, notes, prerelease):
    name = f'mailcraft-{version}-windows-x64-content-runtime.zip'
    if (repository != 'Darkaxt/mailcraft' or binding.get('repository') != repository
            or binding.get('tag') != 'v' + version or binding.get('name') != name
            or any(type(binding.get(key)) is not int or binding[key] <= 0 for key in ('releaseId', 'assetId', 'bytes'))
            or not isinstance(binding.get('sha256'), str) or not re.fullmatch(r'[0-9a-f]{64}', binding['sha256'])
            or not re.fullmatch(r'[0-9a-f]{40}', commit)):
        raise ValueError('Require the exact committed fork release/input identity')
    files = sorted(directory.iterdir())
    if not files or any(not path.is_file() or path.is_symlink() for path in files):
        raise ValueError('Release artifacts must be regular, flat files')
    local = {path.name: dict(path=path, size=path.stat().st_size, digest='sha256:' + digest(path)) for path in files}
    runtime = local.get(name)
    if not runtime or runtime['size'] != binding['bytes'] or runtime['digest'] != 'sha256:' + binding['sha256']:
        raise ValueError('The delivered runtime ZIP differs from the committed input')
    sums = directory / 'SHA256SUMS.txt'
    if not sums.is_file() or sums.read_text(encoding='utf-8').splitlines().count(binding['sha256'] + '  ' + name) != 1:
        raise ValueError('Checksums must bind the unchanged runtime input')
    endpoint = f'repos/{repository}/releases/{binding["releaseId"]}'
    upload_base = f'https://uploads.github.com/repos/{repository}/releases/{binding["releaseId"]}/assets'

    def inspect():
        release = api(endpoint)
        if (release.get('id') != binding['releaseId'] or release.get('draft') is not True
                or release.get('tag_name') != binding['tag'] or release.get('target_commitish') != commit
                or release.get('name') != 'MailCraft v' + version or release.get('prerelease') != prerelease
                or (release.get('body') or '').strip() != notes.strip()
                or release.get('upload_url') != upload_base + '{?name,label}'):
            raise ValueError('The exact prepared draft/source/notes changed; preserve it')
        current = assets(repository, binding['releaseId'])
        selected = [item for item in current if item.get('id') == binding['assetId'] or item.get('name') == name]
        if (len(selected) != 1 or selected[0].get('id') != binding['assetId'] or selected[0].get('name') != name
                or selected[0].get('size') != binding['bytes'] or selected[0].get('digest') != 'sha256:' + binding['sha256']
                or selected[0].get('state') != 'uploaded'):
            raise ValueError('The source-pinned runtime asset changed; never delete or replace it')
        if len({item['name'] for item in current}) != len(current) or set(item['name'] for item in current) - local.keys():
            raise ValueError('Unexpected draft assets are preserved, not swept')
        return current

    inspect()
    for filename, member in local.items():
        if filename == name:
            continue
        previous = next((item for item in inspect() if item['name'] == filename), None)
        if previous:
            if previous.get('size') == member['size'] and previous.get('digest') == member['digest'] and previous.get('state') == 'uploaded':
                continue
            print(json.dumps(dict(operation='replace generated draft output', assetId=previous['id'], name=filename)), flush=True)
            api(f'repos/{repository}/releases/assets/{previous["id"]}', method='DELETE')
        print(json.dumps(dict(operation='upload generated draft output', releaseId=binding['releaseId'], name=filename)), flush=True)
        uploaded = api(upload_base + '?' + urlencode(dict(name=filename)), method='POST', upload=member['path'])
        if any(uploaded.get(key) != value for key, value in dict(name=filename, size=member['size'], digest=member['digest'], state='uploaded').items()):
            raise ValueError('Uploaded asset identity differs; inspect the original operation without replay')
    final = inspect()
    if set(item['name'] for item in final) != local.keys() or any(
            item.get('size') != local[item['name']]['size'] or item.get('digest') != local[item['name']]['digest']
            or item.get('state') != 'uploaded' for item in final):
        raise ValueError('Final draft asset set is incomplete or changed')
    return dict(releaseId=binding['releaseId'], sourceCommit=commit, draft=True, published=False,
                preservedRuntimeAssetId=binding['assetId'], assets=[dict(id=item['id'], name=item['name'], digest=item['digest']) for item in final])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--artifacts', required=True, type=Path)
    parser.add_argument('--notes', required=True, type=Path)
    parser.add_argument('--prerelease', choices=['true', 'false'], required=True)
    args = parser.parse_args()
    selection = json.loads((ROOT / 'native/craft-content/release-runtime.json').read_text(encoding='utf-8'))
    result = refresh(selection['releaseInput'], args.repository, args.version, args.commit,
                     args.artifacts, args.notes.read_text(encoding='utf-8'), args.prerelease == 'true')
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    main()
