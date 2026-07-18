param(
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$SourceCommit
)

$ErrorActionPreference = "Stop"
$githubArchive = "target/distrib/jux-$Target.zip"
$npmArchive = "target/distrib/jux-npm-$Target.zip"
if (-not (Test-Path $githubArchive)) {
    throw "release archive not found: $githubArchive"
}

$workDirectory = Join-Path $env:RUNNER_TEMP "jux-brand-$Target"
Remove-Item $workDirectory -Recurse -Force -ErrorAction SilentlyContinue
$archiveDirectory = Join-Path $workDirectory "archive"
New-Item -ItemType Directory -Path $archiveDirectory | Out-Null
Expand-Archive -Path $githubArchive -DestinationPath $archiveDirectory
$binary = Get-ChildItem $archiveDirectory -Filter "jux.exe" -Recurse | Select-Object -First 1
if ($null -eq $binary) {
    throw "jux archive layout is invalid: $githubArchive"
}

# Preserve the unbranded build outside the archive root so all channel archives derive from one
# Rust compilation.
$baseBinary = Join-Path $workDirectory "jux.base.exe"
Copy-Item $binary.FullName $baseBinary

function Write-BrandedArchive {
    param(
        [string]$Archive,
        [string]$Channel,
        [string]$Installer,
        [string]$ExpectedChannel,
        [string]$ExpectedInstaller
    )

    $branded = "$($binary.FullName).branded.exe"
    & $baseBinary distribution inject `
        --input $baseBinary `
        --output-path $branded `
        --channel $Channel `
        --installer $Installer `
        --version $Version `
        --source-commit $SourceCommit
    Move-Item $branded $binary.FullName -Force

    $metadata = & $binary.FullName distribution show
    if ($metadata -notmatch "channel: $ExpectedChannel" -or $metadata -notmatch "installer: $ExpectedInstaller") {
        throw "distribution metadata verification failed for $Archive"
    }

    $temporaryArchive = "$Archive.tmp.zip"
    Remove-Item $temporaryArchive -Force -ErrorAction SilentlyContinue
    # Compress the extracted contents instead of assuming cargo-dist uses a top-level directory.
    # Windows archives are currently flat, while this also preserves a future nested layout.
    Compress-Archive -Path (Join-Path $archiveDirectory "*") -DestinationPath $temporaryArchive
    Move-Item $temporaryArchive $Archive -Force

    $digest = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
    $name = Split-Path $Archive -Leaf
    [IO.File]::WriteAllText("$Archive.sha256", "$digest *$name`n", (New-Object System.Text.UTF8Encoding $false))
}

Write-BrandedArchive $githubArchive "github-release" "power-shell" "GithubRelease" "PowerShell"
Write-BrandedArchive $npmArchive "npm" "npm" "Npm" "Npm"

Remove-Item $workDirectory -Recurse -Force
