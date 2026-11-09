[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ScratchRoot,
    [Parameter(Mandatory)][string]$Registration,
    [Parameter(Mandatory)][string]$Cargo,
    [Parameter(Mandatory)][string]$Rustc,
    [Parameter(Mandatory)][string]$CargoHome,
    [string]$ArchivesRoot='D:\BuildCache\Pathweft\office\sources',
    [ValidateRange(1,99)][int]$Attempt=1
)
$ErrorActionPreference='Stop'
$scratch=[IO.Path]::GetFullPath($ScratchRoot)
$registered=Get-Content -LiteralPath $Registration -Raw | ConvertFrom-Json
if($registered.scratchRoot -cne $scratch -or !$registered.cleanupTransaction){throw 'Record the exact registered native scratch root first.'}
if((Split-Path -Parent $scratch) -ne 'D:\Temp' -or (Split-Path -Leaf $scratch) -notlike 'mailcraft-index-*'){throw 'Use exact task-owned D: native compiler scratch.'}
$inputs=Get-Content -LiteralPath (Join-Path $scratch 'source-inputs.json') -Raw | ConvertFrom-Json
if(!$inputs.sourcePreparation -or $inputs.buildOrValidation){throw 'Prepare pinned source-only inputs before stage-end compilation.'}
$receiptRoot=Join-Path $scratch 'receipts'
[void][IO.Directory]::CreateDirectory($receiptRoot)
$names=@('CARGO_HOME','CARGO_TARGET_DIR','CARGO_BUILD_JOBS','CARGO_INCREMENTAL','CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS','RUSTC','TEMP','TMP','CRAFT_FONTS_DIR','CRAFT_FONTS_REQUIRED')
$previous=@{}
foreach($name in $names){$previous[$name]=[Environment]::GetEnvironmentVariable($name,'Process')}
try{
    $env:CARGO_HOME=[IO.Path]::GetFullPath($CargoHome)
    $env:CARGO_TARGET_DIR=Join-Path $scratch 'target/native'
    $env:CARGO_BUILD_JOBS='1'
    $env:CARGO_INCREMENTAL='0'
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS='-C target-feature=+crt-static'
    $env:RUSTC=[IO.Path]::GetFullPath($Rustc)
    $env:TEMP=Join-Path $scratch 'process-temp'
    $env:TMP=$env:TEMP
    [void][IO.Directory]::CreateDirectory($env:TEMP)
    if((Get-FileHash -LiteralPath (Join-Path $scratch 'contract/Cargo.toml') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $inputs.contractManifestSha256){throw 'Prepared contract manifest changed.'}
    foreach($file in $inputs.contractInputs){
        $path=Join-Path $scratch ('contract/src/'+(Split-Path -Leaf $file.path))
        if((Get-Item -LiteralPath $path).LinkType -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256){throw 'Prepared contract source changed.'}
    }
    $locks=@{}
    $lockHashes=@{}
    foreach($app in $inputs.apps){
        $engine=$app.engine
        $bridgeRoot=Join-Path $scratch "sources/$engine/apps/mailcraft-content"
        foreach($file in $app.workerInputs){
            $path=Join-Path $bridgeRoot $file.path
            if((Get-Item -LiteralPath $path).LinkType -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256){throw "Prepared $engine bridge input changed."}
        }
        $suffix=if($Attempt -eq 1){''}else{"-attempt$Attempt"}
        $metadataLog=Join-Path $scratch "core-$engine-dependencies$suffix.log"
        if(Test-Path -LiteralPath $metadataLog){throw 'Preserve the original dependency-resolution result before resuming.'}
        & $Cargo metadata --format-version 1 --filter-platform x86_64-pc-windows-msvc --manifest-path (Join-Path $bridgeRoot 'Cargo.toml') *> $metadataLog
        if($LASTEXITCODE -ne 0){throw "Inspect the original $engine dependency result: $metadataLog"}
        $lockResult=& python -B (Join-Path $PSScriptRoot 'check-donor-lock.py') --engine $engine --scratch-root $scratch --archives-root $ArchivesRoot
        if($LASTEXITCODE -ne 0){throw "The $engine donor graph changed; no native compilation started."}
        $locks[$engine]=$lockResult
        $lockHashes[$engine]=(Get-FileHash -LiteralPath (Join-Path $scratch "sources/$engine/Cargo.lock") -Algorithm SHA256).Hash.ToLowerInvariant()
        Write-Output "Pinned $engine donor graph verified before compilation."
    }
    foreach($app in $inputs.apps){
        $engine=$app.engine
        $bridgeRoot=Join-Path $scratch "sources/$engine/apps/mailcraft-content"
        foreach($file in $app.workerInputs){
            $path=Join-Path $bridgeRoot $file.path
            if((Get-Item -LiteralPath $path).LinkType -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256){throw "Prepared $engine bridge input changed."}
        }
        $receiptPath=Join-Path $receiptRoot "$engine-build.json"
        if(Test-Path -LiteralPath $receiptPath){
            $prior=Get-Content -LiteralPath $receiptPath -Raw|ConvertFrom-Json
            if($prior.buildExit -ne 0 -or !$prior.log -or
                ($prior.input|ConvertTo-Json -Depth 50 -Compress) -cne ($app|ConvertTo-Json -Depth 50 -Compress) -or
                ($prior.contractInputs|ConvertTo-Json -Depth 50 -Compress) -cne ($inputs.contractInputs|ConvertTo-Json -Depth 50 -Compress) -or
                $prior.contractManifestSha256 -ne $inputs.contractManifestSha256 -or
                (Get-FileHash -LiteralPath $Rustc -Algorithm SHA256).Hash.ToLowerInvariant() -ne $prior.compilerSha256 -or
                (Get-FileHash -LiteralPath $Cargo -Algorithm SHA256).Hash.ToLowerInvariant() -ne $prior.cargoSha256 -or
                (Get-FileHash -LiteralPath $prior.log -Algorithm SHA256).Hash.ToLowerInvariant() -ne $prior.logSha256 -or
                (Get-Item -LiteralPath $prior.executable).Length -ne $prior.bytes -or
                (Get-FileHash -LiteralPath $prior.executable -Algorithm SHA256).Hash.ToLowerInvariant() -ne $prior.sha256){
                throw "Preserve changed completed $engine inputs/payload; its prior build cannot be reused."
            }
            Write-Output "Reused exact completed $engine build, source, compiler and payload identities verified."
            continue
        }
        $bridge=Join-Path $scratch "sources/$engine/apps/mailcraft-content/Cargo.toml"
        $env:CRAFT_FONTS_DIR=$null
        $env:CRAFT_FONTS_REQUIRED=$null
        if($app.fonts){
            $env:CRAFT_FONTS_DIR=Join-Path $scratch "fonts/$engine"
            $env:CRAFT_FONTS_REQUIRED='1'
            if(!(Test-Path -LiteralPath (Join-Path $env:CRAFT_FONTS_DIR 'fonts/manifest.txt'))){throw "Pinned font inputs unavailable for $engine"}
        }
        $suffix=if($Attempt -eq 1){''}else{"-attempt$Attempt"}
        $log=Join-Path $scratch "core-$engine-build$suffix.log"
        if(Test-Path -LiteralPath $log){throw "Preserve the original build log and result before resuming $engine"}
        & $Cargo build --locked --release --jobs 1 --target x86_64-pc-windows-msvc --package "mailcraft-content-$engine" --manifest-path $bridge *> $log
        $buildExit=$LASTEXITCODE
        if($buildExit -ne 0){throw "Original $engine build exited $buildExit. Inspect $log; no duplicate is launched."}
        if((Get-FileHash -LiteralPath (Join-Path $scratch "sources/$engine/Cargo.lock") -Algorithm SHA256).Hash.ToLowerInvariant() -ne $lockHashes[$engine]){throw 'The inspected donor lock changed during its locked build.'}
        $lockResult=$locks[$engine]
        $binary=Join-Path $env:CARGO_TARGET_DIR "x86_64-pc-windows-msvc/release/mailcraft-content-$engine.exe"
        $receipt=[ordered]@{
            engine=$engine;source=$app.donor;input=$app;contractInputs=$inputs.contractInputs;contractManifestSha256=$inputs.contractManifestSha256
            executable=$binary;bytes=(Get-Item -LiteralPath $binary).Length
            sha256=(Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
            buildExit=$buildExit;lock=($lockResult|ConvertFrom-Json);lockFileSha256=$lockHashes[$engine];log=$log
            compilerSha256=(Get-FileHash -LiteralPath $Rustc -Algorithm SHA256).Hash.ToLowerInvariant()
            cargoSha256=(Get-FileHash -LiteralPath $Cargo -Algorithm SHA256).Hash.ToLowerInvariant()
            logSha256=(Get-FileHash -LiteralPath $log -Algorithm SHA256).Hash.ToLowerInvariant()
            acceptance='Build/input evidence only; native and mailbox workflow qualification remains required.'
        }
        $receipt|ConvertTo-Json -Depth 30|Set-Content -LiteralPath (Join-Path $receiptRoot "$engine-build.json") -Encoding utf8
        [pscustomobject]$receipt|Select-Object engine,executable,sha256,buildExit|ConvertTo-Json
    }
}finally{
    foreach($name in $names){[Environment]::SetEnvironmentVariable($name,$previous[$name],'Process')}
}
