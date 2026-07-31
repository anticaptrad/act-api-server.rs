$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  throw "Node.js 22+ is required."
}
$major = [int]((node -p "process.versions.node.split('.')[0]").Trim())
if ($major -lt 22) {
  throw "Node.js 22+ is required; found $(node -v)."
}

npm ci
npm run check
npm run bind

Write-Host ""
Write-Host "1. Enable the Apps Script API at https://script.google.com/home/usersettings"
Write-Host "2. Run: npm run login"
Write-Host "3. Confirm the account: npm run auth:status"
Write-Host "4. Validate target/profile/push set: npm run preflight"
Write-Host "5. Run the guarded push: npm run push"
Write-Host "6. Run: npm run open"
Write-Host "7. Create/update the owner-only deployment in the Apps Script editor"
Write-Host "8. For public HTTP: npm run profile:http-api; npm run deploy:http-api"
Write-Host "9. Complete First-run setup with the owning Google account"
