$ErrorActionPreference = 'Stop'

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine('error: cargo was not found on PATH. Install Rust from https://rustup.rs and re-run this script.')
    exit 1
}

function Quote-Arg([string]$value) {
    if ($value -match '\s') { return '"' + $value + '"' }
    return $value
}

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    & cargo install --path crates/uriel-cli --locked --force
    if ($LASTEXITCODE -ne 0) {
        [Console]::Error.WriteLine("error: cargo install failed with exit code $LASTEXITCODE")
        exit $LASTEXITCODE
    }
    $binRoot = if ($env:CARGO_INSTALL_ROOT) { $env:CARGO_INSTALL_ROOT } elseif ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $uriel = Join-Path $binRoot 'bin\uriel.exe'
    if (-not (Test-Path -LiteralPath $uriel)) {
        $found = Get-Command uriel -ErrorAction SilentlyContinue
        if (-not $found) {
            [Console]::Error.WriteLine("error: uriel was installed but could not be found under $binRoot\bin; add it to PATH")
            exit 1
        }
        $uriel = $found.Source
    }
    $uriel = (Resolve-Path -LiteralPath $uriel).Path
    & $uriel
    if ($LASTEXITCODE -ne 0) {
        [Console]::Error.WriteLine("error: $uriel exited with code $LASTEXITCODE")
        exit $LASTEXITCODE
    }
    $db = Join-Path $env:USERPROFILE '.seal\graph.seal'
    Write-Output ''
    Write-Output 'Register the MCP server with Claude Code:'
    Write-Output ('claude mcp add uriel -s user -- ' + (Quote-Arg $uriel) + ' mcp --db ' + (Quote-Arg $db))
    exit 0
}
finally {
    Pop-Location
}
