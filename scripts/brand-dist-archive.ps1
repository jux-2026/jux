param(
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$SourceCommit
)

$ErrorActionPreference = "Stop"
$archive = "target/distrib/jux-$Target.zip"
$checksum = "$archive.sha256"
if (-not (Test-Path $archive)) {
    throw "release archive not found: $archive"
}

$workDirectory = Join-Path $env:RUNNER_TEMP "jux-brand-$Target"
Remove-Item $workDirectory -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive -Path $archive -DestinationPath $workDirectory
$binary = Get-ChildItem $workDirectory -Filter "jux.exe" -Recurse | Select-Object -First 1
if ($null -eq $binary) {
    throw "jux.exe not found in $archive"
}

$branded = "$($binary.FullName).branded.exe"
& $binary.FullName distribution inject `
    --input $binary.FullName `
    --output-path $branded `
    --channel github-release `
    --installer power-shell `
    --version $Version `
    --source-commit $SourceCommit
Move-Item $branded $binary.FullName -Force
$metadata = & $binary.FullName distribution show
if ($metadata -notmatch "channel: GithubRelease" -or $metadata -notmatch "installer: PowerShell") {
    throw "injected distribution metadata verification failed"
}

$root = Get-ChildItem $workDirectory -Directory | Select-Object -First 1
$temporaryArchive = "$archive.tmp.zip"
Remove-Item $temporaryArchive -Force -ErrorAction SilentlyContinue
Compress-Archive -Path $root.FullName -DestinationPath $temporaryArchive
Move-Item $temporaryArchive $archive -Force
$digest = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
$name = Split-Path $archive -Leaf
[IO.File]::WriteAllText($checksum, "$digest *$name`n", (New-Object System.Text.UTF8Encoding $false))
