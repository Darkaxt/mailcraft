[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ScratchRoot,
    [Parameter(Mandatory)][string]$Registration,
    [Parameter(Mandatory)][string]$Vcpkg,
    [string]$VisualStudio='',
    [ValidateRange(1,99)][int]$Attempt=1
)
$ErrorActionPreference='Stop'
$scratch=[IO.Path]::GetFullPath($ScratchRoot)
$registered=Get-Content -LiteralPath $Registration -Raw|ConvertFrom-Json
if($registered.scratchRoot -cne $scratch -or !$registered.cleanupTransaction){throw 'Record this exact registered native scratch root first.'}
if((Split-Path -Parent $scratch) -ne 'D:\Temp' -or (Split-Path -Leaf $scratch) -notlike 'mailcraft-index-*'){throw 'Use exact task-owned D: native compiler scratch.'}
$ocr=Join-Path $scratch 'ocr'
$inputs=Get-Content -LiteralPath (Join-Path $ocr 'source-inputs.json') -Raw|ConvertFrom-Json
if(!$inputs.sourcePreparation -or $inputs.nativeExecution){throw 'Prepare source-only OCR inputs before the complete-stage build.'}
if([IO.Path]::GetFullPath($Vcpkg) -cne (Join-Path $inputs.vcpkg.root 'vcpkg.exe')){throw 'Use only the prepared official vcpkg checkout and executable.'}
# Reusing prepared source must verify the unchanged checkout/overlay/triplet,
# source pins, models and notices. The preparation helper's retain/obtain checks
# reject changed preimages; no recognition or compiler process runs here.
& python -B (Join-Path $PSScriptRoot 'prepare-ocr.py') --scratch-root $scratch --registration $Registration --vcpkg-root $inputs.vcpkg.root
if($LASTEXITCODE -ne 0){throw 'Prepared OCR source, build configuration or model identities changed; preserve the original inputs.'}
$suffix=if($Attempt -eq 1){''}else{"-attempt$Attempt"}
$log=Join-Path $scratch "ocr-build$suffix.log"
if(Test-Path -LiteralPath $log){throw 'Inspect the original OCR build and its result before selecting a supported resume.'}
if(!$VisualStudio){
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $found=@(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
    if($LASTEXITCODE -ne 0 -or $found.Count -ne 1){throw 'Resolve exactly one installed Visual Studio instance containing the C++ toolset.'}
    $VisualStudio=$found[0].Trim()
}
$VisualStudio=[IO.Path]::GetFullPath($VisualStudio).TrimEnd('\')
$vcvars=Join-Path $VisualStudio 'VC/Auxiliary/Build/vcvarsall.bat'
$toolset=(Get-Content -LiteralPath (Join-Path $VisualStudio 'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt') -Raw).Trim()
$compiler=Join-Path $VisualStudio "VC/Tools/MSVC/$toolset/bin/Hostx64/x64/cl.exe"
if(!(Test-Path -LiteralPath $vcvars -PathType Leaf) -or !(Test-Path -LiteralPath $compiler -PathType Leaf)){throw 'The selected installed C++ toolset is incomplete; preserve the original build result.'}
$triplet='mailcraft-x64-explicit'
$tripletSource=Join-Path $PSScriptRoot "triplets/$triplet.cmake"
$tripletFile=Join-Path $ocr "triplets/$triplet.cmake"
$tripletHash=(Get-FileHash -LiteralPath $tripletSource -Algorithm SHA256).Hash.ToLowerInvariant()
if(Test-Path -LiteralPath $tripletFile){
    if((Get-Item -LiteralPath $tripletFile).LinkType -or (Get-FileHash -LiteralPath $tripletFile -Algorithm SHA256).Hash.ToLowerInvariant() -ne $tripletHash){throw 'Preserve independently changed explicit compiler triplet inputs.'}
}else{Copy-Item -LiteralPath $tripletSource -Destination $tripletFile}
$environmentScript=Join-Path $PSScriptRoot 'build-ocr-env.cmd'
$names=@('VCPKG_MAX_CONCURRENCY','VCPKG_DISABLE_METRICS','VCPKG_BINARY_SOURCES','VCPKG_VISUAL_STUDIO_PATH','MAILCRAFT_OCR_BASE_PATH','CMAKE_BUILD_PARALLEL_LEVEL','_CL_','TEMP','TMP')
$previous=@{}
foreach($name in $names){$previous[$name]=[Environment]::GetEnvironmentVariable($name,'Process')}
try{
    $env:VCPKG_MAX_CONCURRENCY='1'
    $env:CMAKE_BUILD_PARALLEL_LEVEL='1'
    $env:VCPKG_DISABLE_METRICS='1'
    $env:VCPKG_VISUAL_STUDIO_PATH=$VisualStudio
    $gitDirectory=Split-Path -Parent (Get-Command git.exe -ErrorAction Stop).Source
    $env:MAILCRAFT_OCR_BASE_PATH=@([Environment]::SystemDirectory,$env:SystemRoot,
        (Join-Path $env:SystemRoot 'System32/Wbem'),(Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0'),
        $PSHOME,$gitDirectory) -join ';'
    # This disposable build must not populate the default C: binary cache.
    # Its usable output is retained by assembly, then exact scratch is cleaned.
    $env:VCPKG_BINARY_SOURCES='clear'
    $env:_CL_=($previous['_CL_']+' /MP1').Trim()
    $env:TEMP=Join-Path $scratch 'process-temp'
    $env:TMP=$env:TEMP
    [void][IO.Directory]::CreateDirectory($env:TEMP)
    & $env:ComSpec /d /c $environmentScript $vcvars $Vcpkg $scratch $toolset *> $log
    $buildExit=$LASTEXITCODE
    if($buildExit -ne 0){throw "The original OCR build exited $buildExit. Inspect $log before correction; no duplicate is launched."}
    $binary=Join-Path $ocr "installed/$triplet/tools/tesseract/tesseract.exe"
    $receipt=[ordered]@{
        version=$inputs.sourcePins.tesseract.version;source=$inputs;executable=$binary
        bytes=(Get-Item -LiteralPath $binary).Length
        sha256=(Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
        vcpkgSha256=(Get-FileHash -LiteralPath $Vcpkg -Algorithm SHA256).Hash.ToLowerInvariant()
        buildScriptSha256=(Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
        visualStudio=$VisualStudio;toolset=$toolset;compilerSha256=(Get-FileHash -LiteralPath $compiler -Algorithm SHA256).Hash.ToLowerInvariant()
        triplet=$triplet;tripletSha256=$tripletHash;environmentScriptSha256=(Get-FileHash -LiteralPath $environmentScript -Algorithm SHA256).Hash.ToLowerInvariant()
        childBasePath=$env:MAILCRAFT_OCR_BASE_PATH
        buildExit=$buildExit;logSha256=(Get-FileHash -LiteralPath $log -Algorithm SHA256).Hash.ToLowerInvariant()
        acceptance='Build/input evidence only; actual OCR output and gate-owned workflow qualification remain required.'
    }
    [void][IO.Directory]::CreateDirectory((Join-Path $scratch 'receipts'))
    $receipt|ConvertTo-Json -Depth 50|Set-Content -LiteralPath (Join-Path $scratch 'receipts/ocr-build.json') -Encoding utf8
    [pscustomobject]$receipt|Select-Object version,executable,sha256,buildExit|ConvertTo-Json
}finally{
    foreach($name in $names){[Environment]::SetEnvironmentVariable($name,$previous[$name],'Process')}
}
