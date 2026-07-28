function ok_(data) {
  return { ok: true, data: data === undefined ? null : data };
}

function fail_(message, code, details) {
  const error = new Error(message);
  error.code = code || 'APP_ERROR';
  if (details !== undefined) error.details = details;
  throw error;
}

function sanitizeErrorDetails_(value, depth) {
  depth = Number(depth || 0);
  if (value === null || value === undefined) return null;
  if (depth > 4) return '[truncated]';
  if (Array.isArray(value)) {
    return value.slice(0, 20).map(function(item) {
      return sanitizeErrorDetails_(item, depth + 1);
    });
  }
  if (typeof value === 'object') {
    const clean = {};
    Object.keys(value).slice(0, 50).forEach(function(key) {
      if (/token|secret|authorization|api.?key|upload.?url|body|content/i.test(key)) {
        clean[key] = '[redacted]';
      } else {
        clean[key] = sanitizeErrorDetails_(value[key], depth + 1);
      }
    });
    return clean;
  }
  if (typeof value === 'string') return value.slice(0, 500);
  if (typeof value === 'number' || typeof value === 'boolean') return value;
  return String(value).slice(0, 500);
}

function serializeError_(error) {
  return {
    message: error && error.message ? String(error.message).slice(0, 500) : String(error).slice(0, 500),
    code: error && error.code ? String(error.code).slice(0, 100) : 'UNEXPECTED_ERROR',
    details: error && error.details !== undefined
      ? sanitizeErrorDetails_(error.details, 0)
      : null
  };
}

function withResult_(fn) {
  try {
    return ok_(fn());
  } catch (error) {
    console.error(error && error.stack ? error.stack : error);
    return { ok: false, error: serializeError_(error) };
  }
}

function jsonOutput_(value) {
  return ContentService
    .createTextOutput(JSON.stringify(value))
    .setMimeType(ContentService.MimeType.JSON);
}

function parseJsonBody_(e) {
  if (!e || !e.postData || !e.postData.contents) return {};
  try {
    return JSON.parse(e.postData.contents);
  } catch (error) {
    fail_('Request body must be valid JSON.', 'INVALID_JSON');
  }
}

function getScriptProperties_() {
  return PropertiesService.getScriptProperties();
}

function getConfigValue_(key, fallback) {
  const value = getScriptProperties_().getProperty(key);
  return value === null || value === '' ? fallback : value;
}

function setConfigValues_(values) {
  const clean = {};
  Object.keys(values || {}).forEach(function(key) {
    const value = values[key];
    if (value !== undefined && value !== null) clean[key] = String(value);
  });
  getScriptProperties_().setProperties(clean, false);
}

function isTrue_(value) {
  return String(value).toLowerCase() === 'true';
}

function requiredString_(value, fieldName, maxLength) {
  const text = String(value === undefined || value === null ? '' : value).trim();
  if (!text) fail_(fieldName + ' is required.', 'VALIDATION_ERROR', { field: fieldName });
  if (maxLength && text.length > maxLength) {
    fail_(fieldName + ' exceeds ' + maxLength + ' characters.', 'VALIDATION_ERROR', { field: fieldName });
  }
  return text;
}

function optionalString_(value, maxLength) {
  const text = String(value === undefined || value === null ? '' : value).trim();
  if (maxLength && text.length > maxLength) {
    fail_('Value exceeds ' + maxLength + ' characters.', 'VALIDATION_ERROR');
  }
  return text;
}

function normalizeTags_(value) {
  const values = Array.isArray(value) ? value : String(value || '').split(',');
  const seen = {};
  return values
    .map(function(tag) { return String(tag).trim(); })
    .filter(function(tag) {
      if (!tag || seen[tag.toLowerCase()]) return false;
      seen[tag.toLowerCase()] = true;
      return true;
    })
    .slice(0, 50);
}

function formatDate_(date) {
  return Utilities.formatDate(date, Session.getScriptTimeZone(), 'yyyy-MM-dd');
}

function isoNow_() {
  return new Date().toISOString();
}

function uuid_() {
  return Utilities.getUuid();
}

function sha256Hex_(text) {
  const bytes = Utilities.computeDigest(
    Utilities.DigestAlgorithm.SHA_256,
    String(text),
    Utilities.Charset.UTF_8
  );
  return bytes.map(function(byte) {
    const normalized = byte < 0 ? byte + 256 : byte;
    return ('0' + normalized.toString(16)).slice(-2);
  }).join('');
}

function constantTimeEquals_(left, right) {
  left = String(left || '');
  right = String(right || '');
  if (left.length !== right.length) return false;
  let result = 0;
  for (let i = 0; i < left.length; i += 1) {
    result |= left.charCodeAt(i) ^ right.charCodeAt(i);
  }
  return result === 0;
}

function findHeader_(headers, name) {
  const target = String(name).toLowerCase();
  const keys = Object.keys(headers || {});
  for (let i = 0; i < keys.length; i += 1) {
    if (keys[i].toLowerCase() === target) return headers[keys[i]];
  }
  return null;
}

function responseJson_(response, context) {
  const code = response.getResponseCode();
  const text = response.getContentText();
  if (code < 200 || code >= 300) {
    fail_((context || 'Google API request') + ' failed with HTTP ' + code + '.', 'GOOGLE_API_ERROR', {
      status: code,
      body: text.slice(0, 4000)
    });
  }
  return text ? JSON.parse(text) : {};
}

function getOAuthToken_() {
  return ScriptApp.getOAuthToken();
}

function getActiveEmail_() {
  return Session.getActiveUser().getEmail() || Session.getEffectiveUser().getEmail() || '';
}

function assertDateString_(value, fieldName) {
  const text = requiredString_(value, fieldName, 10);
  if (!/^\d{4}-\d{2}-\d{2}$/.test(text)) {
    fail_(fieldName + ' must be YYYY-MM-DD.', 'VALIDATION_ERROR', { field: fieldName });
  }
  return text;
}

function sanitizeFileName_(value) {
  return String(value || '')
    .replace(/[\\/:*?"<>|\u0000-\u001F]/g, '-')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 180) || 'untitled';
}

function bytesToHuman_(bytes) {
  const value = Number(bytes || 0);
  if (value < 1024) return value + ' B';
  const units = ['KB', 'MB', 'GB', 'TB'];
  let current = value / 1024;
  let unit = units[0];
  for (let i = 1; i < units.length && current >= 1024; i += 1) {
    current /= 1024;
    unit = units[i];
  }
  return current.toFixed(current >= 10 ? 1 : 2) + ' ' + unit;
}

function csvEscape_(value) {
  if (value === null || value === undefined) return '';
  const text = String(value);
  return /[",\n\r]/.test(text) ? '"' + text.replace(/"/g, '""') + '"' : text;
}
