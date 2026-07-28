#!/usr/bin/env python3
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXPECTED = {
    'default': ('MYSELF', False),
    'monetization': ('MYSELF', False),
    'partner': ('MYSELF', False),
    'workspace-admin': ('MYSELF', False),
    'http-api': ('ANYONE_ANONYMOUS', True),
}
errors = []

with tempfile.TemporaryDirectory() as tmp:
    work = pathlib.Path(tmp) / 'project'
    shutil.copytree(ROOT, work)
    for profile, (expected_access, expected_public) in EXPECTED.items():
        result = subprocess.run(
            ['node', 'scripts/use-profile.mjs', profile],
            cwd=work,
            capture_output=True,
            text=True,
        )
        if result.returncode:
            errors.append(f'{profile}: profile command failed: {result.stderr}')
            continue
        try:
            manifest = json.loads((work / 'src/appsscript.json').read_text())
        except Exception as exc:
            errors.append(f'{profile}: invalid generated manifest: {exc}')
            continue
        access = manifest.get('webapp', {}).get('access')
        if access != expected_access:
            errors.append(f'{profile}: expected webapp access {expected_access}, got {access}')
        config = (work / 'src/00_Config.gs').read_text()
        expected_literal = f'PUBLIC_HTTP: {str(expected_public).lower()}'
        if expected_literal not in config or f"NAME: '{profile}'" not in config:
            errors.append(f'{profile}: deployment profile marker not updated correctly')

        services = {item.get('userSymbol') for item in manifest.get('dependencies', {}).get('enabledAdvancedServices', [])}
        scopes = set(manifest.get('oauthScopes', []))
        if profile == 'partner':
            if 'YouTubeContentId' not in services or 'https://www.googleapis.com/auth/youtubepartner' not in scopes:
                errors.append('partner: Content ID service/scope missing')
        if profile == 'workspace-admin':
            if 'AdminDirectory' not in services or 'https://www.googleapis.com/auth/admin.directory.user.readonly' not in scopes:
                errors.append('workspace-admin: Admin Directory service/scope missing')
        if profile == 'monetization' and 'https://www.googleapis.com/auth/yt-analytics-monetary.readonly' not in scopes:
            errors.append('monetization: monetary analytics scope missing')

if errors:
    print('PROFILE VALIDATION FAILED')
    for error in errors:
        print(f'- {error}')
    sys.exit(1)

print('PROFILE VALIDATION PASSED')
for profile in EXPECTED:
    print(f'- {profile}')
