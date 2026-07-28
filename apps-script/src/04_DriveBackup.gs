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

function getDriveFileMetadata_(fileId) {
  fileId = requiredString_(fileId, 'fileId', 300);
  try {
    return Drive.Files.get(fileId, {
      fields: 'id,name,mimeType,size,md5Checksum,modifiedTime,parents,webViewLink,driveId',
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
