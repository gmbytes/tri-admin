param()
$ErrorActionPreference = 'Stop'
$triRepo = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$triImage = 'rust@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f'
$triContainer = 'tri-admin-mcp-linux-' + [guid]::NewGuid().ToString('N')
$triMounts = @(
    @('Cargo.toml', '/work/Cargo.toml'),
    @('Cargo.lock', '/work/Cargo.lock'),
    @('rust-toolchain.toml', '/work/rust-toolchain.toml'),
    @('crates', '/work/crates'),
    @('services/admin-api/Cargo.toml', '/work/services/admin-api/Cargo.toml'),
    @('services/admin-api/src', '/work/services/admin-api/src')
)
$triArgs = @('run', '--rm', '--init', '--name', $triContainer, '--label', 'tri-admin-test=mcp-linux', '--workdir', '/work', '-e', 'CARGO_HOME=/tmp/cargo', '-e', 'CARGO_TARGET_DIR=/tmp/target', '-e', 'CARGO_INCREMENTAL=0', '-e', 'CARGO_BUILD_JOBS=2', '-e', 'CARGO_PROFILE_DEV_DEBUG=0')
foreach ($triMount in $triMounts) {
    $triSource = [System.IO.Path]::GetFullPath((Join-Path $triRepo $triMount[0]))
    if (-not $triSource.StartsWith($triRepo + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing out-of-workspace source mount' }
    if (-not (Test-Path -LiteralPath $triSource)) { throw 'Required source mount is missing' }
    $triArgs += @('--mount', "type=bind,source=$triSource,target=$($triMount[1]),readonly")
}
$triArgs += @($triImage, 'sh', '-c', 'rustc --version && cargo test -p tri-mcp-runtime --locked -j 2 && cargo clippy -p tri-mcp-runtime --all-targets --locked -j 2 -- -D warnings')
& docker @triArgs
exit $LASTEXITCODE
