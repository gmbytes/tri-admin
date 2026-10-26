#requires -Version 7.0
param()
$ErrorActionPreference='Stop'
$triProjectRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$triRunRoot=Join-Path $triProjectRoot ('.superpowers\generated-projects\'+[guid]::NewGuid().ToString('N'))
$triAllowedRoot=Join-Path $triProjectRoot '.superpowers\generated-projects'
if(![IO.Path]::GetFullPath($triRunRoot).StartsWith([IO.Path]::GetFullPath($triAllowedRoot)+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Generated probe path escaped workspace'}
New-Item -ItemType Directory -Path $triRunRoot -Force | Out-Null
$triSource=Join-Path $triRunRoot 'source';$triArchive=Join-Path $triRunRoot 'project.zip';$triOutput=Join-Path $triRunRoot 'unpacked'
$triSchema=Join-Path $triRunRoot 'schema.json'
@'
{"resource":"inventory_items","label":"库存物品","fields":[{"name":"sku","label":"编号","kind":"text","required":true,"searchable":true,"unique":true},{"name":"count","label":"数量","kind":"integer","required":true},{"name":"ratio","label":"比例","kind":"number"},{"name":"enabled","label":"启用","kind":"boolean","required":true},{"name":"releasedAt","label":"发布时间","kind":"datetime"},{"name":"tags","label":"标签","kind":"textList"}]}
'@ | Set-Content -LiteralPath $triSchema -Encoding utf8NoBOM
function Invoke-TriGeneratedCheck([string]$triCommand,[string[]]$triArguments,[string]$triDirectory,[string]$triLogName){
 Push-Location -LiteralPath $triDirectory
 try{& $triCommand @triArguments *> (Join-Path $triRunRoot $triLogName);$triCode=$LASTEXITCODE;if($triCode -ne 0){Get-Content -LiteralPath (Join-Path $triRunRoot $triLogName) -Tail 35;throw "Generated project check failed ($triLogName), evidence: $triRunRoot"}}
 finally{Pop-Location}
}
$triOldTarget=$env:CARGO_TARGET_DIR;$triOldIncremental=$env:CARGO_INCREMENTAL;$triOldUri=$env:TRI_TEST_MONGODB_URI
try{
 $env:CARGO_INCREMENTAL='0';$env:CARGO_TARGET_DIR=Join-Path $triProjectRoot 'target'
 if(!$env:TRI_TEST_MONGODB_URI){$triLine=Get-Content -LiteralPath (Join-Path $triProjectRoot '.env') | Where-Object{$_.StartsWith('TRI_MONGODB_URI=')} | Select-Object -First 1;if(!$triLine){throw 'A test MongoDB replica-set URI is required'};$env:TRI_TEST_MONGODB_URI=$triLine.Substring('TRI_MONGODB_URI='.Length)}
 Invoke-TriGeneratedCheck 'cargo' @('run','--locked','-p','tri-resource-generator','--example','render','--',$triSchema,$triSource,$triArchive) $triProjectRoot 'render.log'
 Add-Type -AssemblyName System.IO.Compression.FileSystem
 $triZip=[IO.Compression.ZipFile]::OpenRead($triArchive)
 try{if($triZip.Entries.Count -gt 64){throw 'Too many generated entries'};foreach($triEntry in $triZip.Entries){if($triEntry.FullName.Contains('\') -or $triEntry.FullName.Contains(':') -or $triEntry.FullName.Split('/').Where({$_ -eq '..' -or $_ -eq ''}).Count -gt 0){throw 'Unsafe archive path'}}}
 finally{$triZip.Dispose()}
 Expand-Archive -LiteralPath $triArchive -DestinationPath $triOutput
 Invoke-TriGeneratedCheck 'node' @((Join-Path $PSScriptRoot 'check-generated-manifest.mjs'),$triOutput) $triProjectRoot 'manifest.log'
 Invoke-TriGeneratedCheck 'cargo' @('fmt','--all','--','--check') $triOutput 'cargo-fmt.log'
 Invoke-TriGeneratedCheck 'cargo' @('build','--workspace','--locked','--offline') $triOutput 'cargo-build.log'
 Invoke-TriGeneratedCheck 'cargo' @('test','--workspace','--locked','--offline') $triOutput 'cargo-test.log'
 $triWeb=Join-Path $triOutput 'web'
 Invoke-TriGeneratedCheck 'npm' @('ci','--ignore-scripts','--offline') $triWeb 'npm-ci.log'
 Invoke-TriGeneratedCheck 'npm' @('test') $triWeb 'npm-test.log'
 Invoke-TriGeneratedCheck 'npm' @('run','build') $triWeb 'npm-build.log'
 $triModules=[IO.Path]::GetFullPath((Join-Path $triWeb 'node_modules'))
 if(!$triModules.StartsWith([IO.Path]::GetFullPath($triRunRoot)+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Refusing out-of-scope dependency cleanup'}
 if(Test-Path -LiteralPath $triModules){if((Get-Item -LiteralPath $triModules).Attributes.HasFlag([IO.FileAttributes]::ReparsePoint)){throw 'Refusing linked dependency cleanup'};Remove-Item -LiteralPath $triModules -Recurse -Force}
 [pscustomobject]@{passed=$true;artifact=$triArchive;project=$triOutput;sha256=(Get-FileHash -LiteralPath $triArchive -Algorithm SHA256).Hash.ToLowerInvariant()} | ConvertTo-Json -Compress
}
finally{$env:CARGO_TARGET_DIR=$triOldTarget;$env:CARGO_INCREMENTAL=$triOldIncremental;$env:TRI_TEST_MONGODB_URI=$triOldUri}
