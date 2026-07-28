#!/usr/bin/env python3
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
web = (ROOT / 'src' / '02_WebApp.gs').read_text()
utils = (ROOT / 'src' / '01_Utils.gs').read_text()
config = (ROOT / 'src' / '00_Config.gs').read_text()
errors = []

def check(condition, message):
    if not condition:
        errors.append(message)

check("VERSION: '1.0.3'" in config, 'Apps Script version must be 1.0.3')
check('Session.getActiveUser().getEmail()' in web, 'UI caller must inspect active user identity')
check('Session.getEffectiveUser().getEmail()' in web, 'UI caller must inspect effective user identity')
check('active && effective && active === effective' in web,
      'UI caller must require matching non-empty active/effective identities')
check("if (DEPLOYMENT_PROFILE.PUBLIC_HTTP) return false" in web,
      'public HTTP profile must disable browser RPC')
check("bootstrap: true" in web and "rotateApiKey: true" in web and "saveConfig: true" in web,
      'configuration and key-management actions must be UI-only')
check("ACTION_NOT_AVAILABLE" in web, 'HTTP action allowlist must fail closed')
check("requiredString_(payload && payload.controlRequestId" in web,
      'HTTP mutations must require a controlRequestId')
check("http.control.requested" in web and "http.control.completed" in web and "http.control.failed" in web,
      'HTTP mutations must emit correlated audit events')
check('stack:' not in re.search(r'function serializeError_\(error\)[\s\S]*?\n}', utils).group(0),
      'serialized HTTP errors must not return stack traces')
check('sanitizeErrorDetails_' in utils and "'[redacted]'" in utils,
      'error details must be recursively redacted')
check('/token|secret|authorization|api.?key|upload.?url|body|content/i' in utils,
      'sensitive error keys must be redacted')

if errors:
    print('HTTP SECURITY VALIDATION FAILED')
    for error in errors:
        print('- ' + error)
    sys.exit(1)

print('HTTP SECURITY VALIDATION PASSED')
print('- anonymous UI/RPC fail closed independent of profile marker')
print('- HTTP actions are allowlisted; management actions are UI-only')
print('- mutating requests require correlation IDs and emit audit events')
print('- returned errors omit stacks and redact sensitive detail fields')
