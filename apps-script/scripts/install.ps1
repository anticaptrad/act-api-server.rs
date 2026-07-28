$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  throw "Node.js 22+ is required."
}
$major = [int]((node -p "process.versions.node.split('.')[0]").Trim())
if ($major -lt 22) {
  throw "Node.js 22+ is required; found $(node -v)."
}

npm install
npm run check

Write-Host ""
Write-Host "1. Enable the Apps Script API at https://script.google.com/home/usersettings"
Write-Host "2. Run: npx clasp login"
Write-Host "3. Run: npm run bind"
Write-Host "4. Optional safety copy: npm run backup:remote"
Write-Host "5. Run: npm run push"
Write-Host "6. Run: npm run open"
Write-Host "7. Deploy as a web app: Execute as Me; access Only myself"
Write-Host "8. Open the web app and click First-run setup"
