function ensureFolderStructure_() {
  const root = findOrCreateFolder_(null, APP.ROOT_FOLDER_NAME);
  const result = { root: folderInfo_(root) };
  Object.keys(APP.FOLDERS).forEach(function(key) {
    const folder = findOrCreateFolder_(root, APP.FOLDERS[key]);
    result[key.toLowerCase()] = folderInfo_(folder);
  });
  setConfigValues_({ ROOT_FOLDER_ID: root.getId() });
  return result;
}

function folderInfo_(folder) {
  return { id: folder.getId(), name: folder.getName(), url: folder.getUrl() };
}

function findOrCreateFolder_(parent, name) {
  const iterator = parent ? parent.getFoldersByName(name) : DriveApp.getFoldersByName(name);
  if (iterator.hasNext()) return iterator.next();
  return parent ? parent.createFolder(name) : DriveApp.createFolder(name);
}

function getFolderByLogicalName_(logicalName) {
  const structure = ensureFolderStructure_();
  const key = String(logicalName || '').toLowerCase();
  if (!structure[key]) fail_('Unknown folder: ' + logicalName, 'CONFIG_ERROR');
  return DriveApp.getFolderById(structure[key].id);
}

function hasExpectedVideoSignature_(bytes, mimeType) {
  if (mimeType === 'video/webm') {
    return bytes.length >= 4 &&
      (bytes[0] & 255) === 0x1a &&
      (bytes[1] & 255) === 0x45 &&
      (bytes[2] & 255) === 0xdf &&
      (bytes[3] & 255) === 0xa3;
  }
  return bytes.length >= 12 &&
    String.fromCharCode(bytes[4] & 255, bytes[5] & 255, bytes[6] & 255, bytes[7] & 255) === 'ftyp';
}

function ingestHttpVideo_(payload) {
  payload = payload || {};
  const fileName = sanitizeFileName_(requiredString_(payload.fileName, 'fileName', 180));
  const mimeType = requiredString_(payload.mimeType, 'mimeType', 100).toLowerCase();
  if (!/^video\/(mp4|webm|quicktime)$/.test(mimeType)) {
    fail_('HTTP video ingest accepts MP4, WebM, or QuickTime media only.', 'INVALID_VIDEO_FILE');
  }

  const maxBase64Chars = Math.ceil(APP.MAX_HTTP_INGEST_BYTES / 3) * 4 + 8;
  const encoded = requiredString_(payload.base64, 'base64', maxBase64Chars).replace(/\s/g, '');
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(encoded)) {
    fail_('Video data must use standard base64 encoding.', 'INVALID_VIDEO_DATA');
  }

  let bytes;
  try {
    bytes = Utilities.base64Decode(encoded);
  } catch (error) {
    fail_('Video data is not valid base64.', 'INVALID_VIDEO_DATA');
  }
  if (!bytes.length || bytes.length > APP.MAX_HTTP_INGEST_BYTES) {
    fail_(
      'Decoded video must be between 1 byte and ' + bytesToHuman_(APP.MAX_HTTP_INGEST_BYTES) + '.',
      'VIDEO_SIZE_LIMIT',
      { decodedBytes: bytes.length, maximumBytes: APP.MAX_HTTP_INGEST_BYTES }
    );
  }
  if (!hasExpectedVideoSignature_(bytes, mimeType)) {
    fail_('Decoded bytes do not match the declared video container.', 'INVALID_VIDEO_SIGNATURE');
  }

  const expectedSha256 = requiredString_(payload.sha256, 'sha256', 64).toLowerCase();
  if (!/^[a-f0-9]{64}$/.test(expectedSha256)) {
    fail_('sha256 must contain exactly 64 lowercase hexadecimal characters.', 'VALIDATION_ERROR');
  }
  const actualSha256 = sha256BytesHex_(bytes);
  if (!constantTimeEquals_(actualSha256, expectedSha256)) {
    fail_('Decoded video does not match the supplied SHA-256 digest.', 'CONTENT_DIGEST_MISMATCH');
  }

  const controlRequestId = requiredString_(payload.controlRequestId, 'controlRequestId', 200);
  const requestHash = sha256Hex_(controlRequestId);
  const idempotencyPropertyKey = APP.HTTP_INGEST_PREFIX + requestHash;
  const storedName = 'http-' + requestHash.slice(0, 16) + '-' + fileName;
  const inbox = getFolderByLogicalName_('inbox');
  const properties = PropertiesService.getScriptProperties();
  const lock = LockService.getScriptLock();
  if (!lock.tryLock(10000)) {
    fail_('Another video ingest is running. Retry with the same controlRequestId.', 'UPLOAD_BUSY');
  }

  try {
    const priorReceiptText = properties.getProperty(idempotencyPropertyKey);
    if (priorReceiptText) {
      let priorReceipt;
      try {
        priorReceipt = JSON.parse(priorReceiptText);
      } catch (error) {
        fail_('The stored ingest receipt is invalid.', 'IDEMPOTENCY_STATE_INVALID');
      }
      if (!constantTimeEquals_(priorReceipt.contentSha256 || '', actualSha256)) {
        fail_(
          'The controlRequestId was already used for different video content.',
          'IDEMPOTENCY_CONFLICT'
        );
      }
      const priorFile = getDriveFileMetadata_(priorReceipt.driveFileId);
      return {
        file: validateVideoFile_(priorFile),
        contentSha256: actualSha256,
        idempotentReplay: true
      };
    }

    const existing = inbox.getFilesByName(storedName);
    if (existing.hasNext()) {
      const existingMetadata = getDriveFileMetadata_(existing.next().getId());
      const properties = existingMetadata.appProperties || {};
      if (!constantTimeEquals_(properties.contentSha256 || '', actualSha256)) {
        fail_(
          'The controlRequestId was already used for different video content.',
          'IDEMPOTENCY_CONFLICT'
        );
      }
      properties.setProperty(idempotencyPropertyKey, JSON.stringify({
        driveFileId: existingMetadata.id,
        contentSha256: actualSha256
      }));
      return {
        file: validateVideoFile_(existingMetadata),
        contentSha256: actualSha256,
        idempotentReplay: true
      };
    }

    const created = Drive.Files.create({
      name: storedName,
      mimeType: mimeType,
      parents: [inbox.getId()],
      description: 'Authenticated HTTP video ingest for Anticaptrad. Created ' + isoNow_(),
      appProperties: {
        anticaptradPurpose: 'http-video-ingest',
        contentSha256: actualSha256,
        controlRequestHash: requestHash
      }
    }, Utilities.newBlob(bytes, mimeType, storedName), {
      supportsAllDrives: true,
      fields: 'id,name,mimeType,size,md5Checksum,modifiedTime,parents,webViewLink,driveId,appProperties'
    });
    const file = validateVideoFile_(created);
    properties.setProperty(idempotencyPropertyKey, JSON.stringify({
      driveFileId: file.id,
      contentSha256: actualSha256
    }));
    writeAuditLog_('drive.http_video_ingested', {
      controlRequestId: controlRequestId,
      driveFileId: file.id,
      sha256: actualSha256,
      size: file.size,
      mimeType: mimeType
    });
    return {
      file: file,
      contentSha256: actualSha256,
      idempotentReplay: false
    };
  } finally {
    lock.releaseLock();
  }
}

function getDriveFileMetadata_(fileId) {
  fileId = requiredString_(fileId, 'fileId', 300);
  try {
    return Drive.Files.get(fileId, {
      fields: 'id,name,mimeType,size,md5Checksum,modifiedTime,parents,webViewLink,driveId,appProperties',
      supportsAllDrives: true
    });
  } catch (error) {
    fail_('Unable to read the Drive file. Confirm the deploying account can access it.', 'DRIVE_FILE_ERROR', {
      fileId: fileId,
      cause: error.message
    });
  }
}

function validateVideoFile_(metadata) {
  if (!metadata) fail_('Drive file metadata is missing.', 'DRIVE_FILE_ERROR');
  if (!String(metadata.mimeType || '').match(/^video\//)) {
    fail_('Drive file must have a video/* MIME type.', 'INVALID_VIDEO_FILE', {
      mimeType: metadata.mimeType,
      name: metadata.name
    });
  }
  const size = Number(metadata.size || 0);
  if (!size || size < 1) fail_('Drive file is empty or its size is unavailable.', 'INVALID_VIDEO_FILE');
  return {
    id: metadata.id,
    name: metadata.name,
    mimeType: metadata.mimeType,
    size: size,
    sizeHuman: bytesToHuman_(size),
    md5Checksum: metadata.md5Checksum || null,
    webViewLink: metadata.webViewLink || null
  };
}

function createSourceBackup_(sourceFile, uploadMetadata) {
  const backupFolder = getFolderByLogicalName_('backups');
  const timestamp = Utilities.formatDate(new Date(), Session.getScriptTimeZone(), 'yyyyMMdd-HHmmss');
  const backupName = timestamp + ' - ' + sanitizeFileName_(sourceFile.name);
  const copied = Drive.Files.copy({
    name: backupName,
    parents: [backupFolder.getId()],
    description: 'Immutable source backup for Anticaptrad YouTube upload. Created ' + isoNow_(),
    appProperties: {
      anticaptradPurpose: 'youtube-source-backup',
      originalFileId: sourceFile.id,
      uploadTitle: String(uploadMetadata.title || '').slice(0, 120)
    }
  }, sourceFile.id, {
    supportsAllDrives: true,
    fields: 'id,name,mimeType,size,md5Checksum,webViewLink,createdTime'
  });
  return validateVideoFile_(copied);
}

function writeJsonFile_(logicalFolder, fileName, value) {
  const folder = getFolderByLogicalName_(logicalFolder);
  const blob = Utilities.newBlob(
    JSON.stringify(value, null, 2),
    'application/json',
    sanitizeFileName_(fileName)
  );
  const file = folder.createFile(blob);
  return { id: file.getId(), name: file.getName(), url: file.getUrl() };
}

function writeTextFile_(logicalFolder, fileName, mimeType, text) {
  const folder = getFolderByLogicalName_(logicalFolder);
  const file = folder.createFile(Utilities.newBlob(String(text), mimeType, sanitizeFileName_(fileName)));
  return { id: file.getId(), name: file.getName(), url: file.getUrl() };
}

function writeAuditLog_(eventType, payload) {
  const record = {
    eventType: eventType,
    timestamp: isoNow_(),
    actor: getActiveEmail_(),
    payload: payload || {}
  };
  const day = Utilities.formatDate(new Date(), Session.getScriptTimeZone(), 'yyyy-MM-dd');
  const fileName = day + '-' + sanitizeFileName_(eventType) + '-' + uuid_().slice(0, 8) + '.json';
  try {
    return writeJsonFile_('logs', fileName, record);
  } catch (error) {
    console.error('Audit log write failed: ' + error.message);
    return null;
  }
}

function downloadDriveRange_(fileId, start, end) {
  const url = 'https://www.googleapis.com/drive/v3/files/' + encodeURIComponent(fileId) +
    '?alt=media&supportsAllDrives=true';
  const response = UrlFetchApp.fetch(url, {
    method: 'get',
    headers: {
      Authorization: 'Bearer ' + getOAuthToken_(),
      Range: 'bytes=' + start + '-' + end
    },
    muteHttpExceptions: true,
    followRedirects: true
  });
  const code = response.getResponseCode();
  if (code !== 200 && code !== 206) {
    fail_('Failed to read a byte range from Drive.', 'DRIVE_RANGE_ERROR', {
      status: code,
      body: response.getContentText().slice(0, 2000),
      start: start,
      end: end
    });
  }
  return response.getBlob().getBytes();
}
