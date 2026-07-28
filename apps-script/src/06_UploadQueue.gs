function startUploadJob_(payload) {
  payload = payload || {};
  const idempotency = reserveUploadIdempotency_(payload.idempotencyKey);
  if (idempotency.existingJob) {
    return {
      job: publicUploadJob_(idempotency.existingJob),
      message: 'The same idempotency key already created this upload job; no duplicate backup or upload was started.'
    };
  }

  let jobId = '';
  try {
    const channel = getMyChannel_();
    assertExpectedChannel_(channel, false);

    const title = requiredString_(payload.title, 'title', 100);
  const description = optionalString_(payload.description, 5000);
  const categoryId = optionalString_(payload.categoryId || '22', 10) || '22';
  const sourceFile = validateVideoFile_(getDriveFileMetadata_(payload.driveFileId));
  if (typeof payload.madeForKids !== 'boolean') {
    fail_('madeForKids must be explicitly true or false.', 'VALIDATION_ERROR');
  }
  if (payload.rightsConfirmed !== true) {
    fail_('You must confirm that Anticaptrad has the rights to upload this source file.', 'RIGHTS_CONFIRMATION_REQUIRED');
  }

  const metadata = {
    title: title,
    description: description,
    tags: normalizeTags_(payload.tags),
    categoryId: categoryId,
    defaultLanguage: optionalString_(payload.defaultLanguage || 'en', 20) || 'en',
    madeForKids: Boolean(payload.madeForKids),
    privacyStatus: 'private',
    playlistId: optionalString_(payload.playlistId, 200),
    thumbnailDriveFileId: optionalString_(payload.thumbnailDriveFileId, 300),
    provenance: {
      repository: optionalString_(payload.repository, 500),
      commit: optionalString_(payload.commit, 100),
      requestedBy: getActiveEmail_(),
      requestedAt: isoNow_()
    }
  };

    const backupFile = createSourceBackup_(sourceFile, metadata);
    jobId = uuid_();
    const resource = buildVideoResource_(metadata);
  const uploadUrl = initiateResumableUpload_(backupFile, resource);
  const now = isoNow_();
  const job = {
    id: jobId,
    status: 'queued',
    createdAt: now,
    updatedAt: now,
    attempts: 0,
    lastError: null,
    uploadUrl: uploadUrl,
    offset: 0,
    totalBytes: backupFile.size,
    chunkBytes: APP.UPLOAD_CHUNK_BYTES,
    sourceFile: sourceFile,
    backupFile: backupFile,
    metadata: metadata,
    youtubeVideo: null,
    artifacts: {}
  };
  saveUploadJob_(job);
  job.artifacts.requestManifest = writeJsonFile_(
    'metadata',
    sanitizeFileName_(title) + '-' + jobId.slice(0, 8) + '-upload-request.json',
    publicUploadJob_(job)
  );
  saveUploadJob_(job);
  writeAuditLog_('youtube.upload.queued', {
    jobId: jobId,
    title: title,
    sourceFileId: sourceFile.id,
    backupFileId: backupFile.id,
    totalBytes: backupFile.size
  });

    commitUploadIdempotency_(idempotency, jobId);
    return {
      job: publicUploadJob_(job),
      message: 'Upload queued as private. The five-minute trigger will continue it automatically.'
    };
  } catch (error) {
    releaseUploadIdempotency_(idempotency, jobId);
    throw error;
  }
}

function reserveUploadIdempotency_(rawKey) {
  const key = optionalString_(rawKey, 200);
  if (!key) return { enabled: false, hash: '', reservation: '', existingJob: null };

  const hash = sha256Hex_(key);
  const propertyKey = APP.IDEMPOTENCY_PREFIX + hash;
  const reservation = 'PENDING:' + uuid_() + ':' + Date.now();
  const lock = LockService.getScriptLock();
  if (!lock.tryLock(5000)) fail_('Another upload request is being registered. Retry with the same idempotency key.', 'UPLOAD_BUSY');
  try {
    const existing = getScriptProperties_().getProperty(propertyKey);
    if (existing && existing.indexOf('JOB:') === 0) {
      const job = getUploadJob_(existing.slice(4));
      if (job) return { enabled: true, hash: hash, reservation: existing, existingJob: job };
      getScriptProperties_().deleteProperty(propertyKey);
    } else if (existing && existing.indexOf('PENDING:') === 0) {
      const parts = existing.split(':');
      const createdAt = Number(parts[2] || 0);
      if (createdAt && Date.now() - createdAt < 15 * 60 * 1000) {
        fail_('An upload with this idempotency key is already being registered. Retry shortly with the same key.', 'IDEMPOTENCY_IN_PROGRESS');
      }
      getScriptProperties_().deleteProperty(propertyKey);
    }
    getScriptProperties_().setProperty(propertyKey, reservation);
    return { enabled: true, hash: hash, reservation: reservation, existingJob: null };
  } finally {
    lock.releaseLock();
  }
}

function commitUploadIdempotency_(idempotency, jobId) {
  if (!idempotency || !idempotency.enabled) return;
  const propertyKey = APP.IDEMPOTENCY_PREFIX + idempotency.hash;
  const lock = LockService.getScriptLock();
  lock.waitLock(10000);
  try {
    const current = getScriptProperties_().getProperty(propertyKey);
    if (current === idempotency.reservation || !current) {
      getScriptProperties_().setProperty(propertyKey, 'JOB:' + jobId);
    }
  } finally {
    lock.releaseLock();
  }
}

function releaseUploadIdempotency_(idempotency, jobId) {
  if (!idempotency || !idempotency.enabled) return;
  const propertyKey = APP.IDEMPOTENCY_PREFIX + idempotency.hash;
  const lock = LockService.getScriptLock();
  lock.waitLock(10000);
  try {
    const current = getScriptProperties_().getProperty(propertyKey);
    if (jobId && getUploadJob_(jobId)) {
      getScriptProperties_().setProperty(propertyKey, 'JOB:' + jobId);
    } else if (current === idempotency.reservation) {
      getScriptProperties_().deleteProperty(propertyKey);
    }
  } finally {
    lock.releaseLock();
  }
}

function buildVideoResource_(metadata) {
  return {
    snippet: {
      title: metadata.title,
      description: metadata.description,
      tags: metadata.tags,
      categoryId: metadata.categoryId,
      defaultLanguage: metadata.defaultLanguage
    },
    status: {
      privacyStatus: 'private',
      selfDeclaredMadeForKids: metadata.madeForKids,
      embeddable: true,
      license: 'youtube'
    }
  };
}

function initiateResumableUpload_(driveFile, resource) {
  const url = 'https://www.googleapis.com/upload/youtube/v3/videos' +
    '?uploadType=resumable&part=snippet,status';
  const response = UrlFetchApp.fetch(url, {
    method: 'post',
    contentType: 'application/json; charset=UTF-8',
    headers: {
      Authorization: 'Bearer ' + getOAuthToken_(),
      'X-Upload-Content-Length': String(driveFile.size),
      'X-Upload-Content-Type': driveFile.mimeType
    },
    payload: JSON.stringify(resource),
    muteHttpExceptions: true,
    followRedirects: false
  });
  const code = response.getResponseCode();
  if (code < 200 || code >= 300) {
    fail_('YouTube refused to initialize the resumable upload.', 'YOUTUBE_UPLOAD_INIT_ERROR', {
      status: code,
      body: response.getContentText().slice(0, 4000)
    });
  }
  const location = findHeader_(response.getHeaders(), 'Location');
  if (!location) fail_('YouTube did not return a resumable upload URL.', 'YOUTUBE_UPLOAD_INIT_ERROR');
  return String(location);
}

function processUploadJob_(jobId, maxChunks) {
  jobId = requiredString_(jobId, 'jobId', 100);
  const lock = LockService.getScriptLock();
  if (!lock.tryLock(5000)) fail_('Another upload operation is already running.', 'UPLOAD_BUSY');
  try {
    let job = getUploadJob_(jobId);
    if (!job) fail_('Upload job not found.', 'JOB_NOT_FOUND');
    if (job.status === 'complete') return publicUploadJob_(job);
    if (job.status === 'cancelled') fail_('Upload job is cancelled.', 'JOB_CANCELLED');

    const limit = Math.min(Math.max(Number(maxChunks || APP.MAX_CHUNKS_PER_RUN), 1), 12);
    const deadline = Date.now() + 4.5 * 60 * 1000;
    job.status = 'uploading';
    job.attempts = Number(job.attempts || 0) + 1;
    job.updatedAt = isoNow_();
    job.lastError = null;
    saveUploadJob_(job);

    job = syncResumableUploadState_(job);
    if (job.status === 'finalizing' && job.youtubeVideo && job.youtubeVideo.id) {
      return publicUploadJob_(finalizeUploadJob_(job));
    }

    let processed = 0;
    while (job.offset < job.totalBytes && processed < limit && Date.now() < deadline) {
      const end = Math.min(job.totalBytes - 1, job.offset + job.chunkBytes - 1);
      const bytes = downloadDriveRange_(job.backupFile.id, job.offset, end);
      const expectedLength = end - job.offset + 1;
      if (bytes.length !== expectedLength) {
        fail_('Drive returned an unexpected byte count.', 'DRIVE_RANGE_ERROR', {
          expected: expectedLength,
          actual: bytes.length,
          start: job.offset,
          end: end
        });
      }

      const uploadResponse = UrlFetchApp.fetch(job.uploadUrl, {
        method: 'put',
        contentType: job.backupFile.mimeType,
        headers: {
          Authorization: 'Bearer ' + getOAuthToken_(),
          'Content-Range': 'bytes ' + job.offset + '-' + end + '/' + job.totalBytes
        },
        payload: bytes,
        muteHttpExceptions: true,
        followRedirects: false
      });
      const code = uploadResponse.getResponseCode();

      if (code === 200 || code === 201) {
        job.offset = job.totalBytes;
        job.youtubeVideo = JSON.parse(uploadResponse.getContentText() || '{}');
        job.status = 'finalizing';
        job.updatedAt = isoNow_();
        saveUploadJob_(job);
        break;
      }

      if (code === 308) {
        const range = findHeader_(uploadResponse.getHeaders(), 'Range');
        const match = String(range || '').match(/bytes=0-(\d+)/);
        job.offset = match ? Number(match[1]) + 1 : 0;
        job.updatedAt = isoNow_();
        saveUploadJob_(job);
        processed += 1;
        continue;
      }

      if (code === 404) {
        job.uploadUrl = initiateResumableUpload_(job.backupFile, buildVideoResource_(job.metadata));
        job.offset = 0;
        job.lastError = 'Resumable session expired and was restarted.';
        job.updatedAt = isoNow_();
        saveUploadJob_(job);
        processed += 1;
        continue;
      }

      if (code >= 500 && code <= 599) {
        job = syncResumableUploadState_(job);
        if (job.status === 'finalizing' && job.youtubeVideo && job.youtubeVideo.id) {
          return publicUploadJob_(finalizeUploadJob_(job));
        }
        job.status = 'queued';
        job.lastError = 'YouTube temporary HTTP ' + code + '; upload position was reconciled and the trigger will retry.';
        job.updatedAt = isoNow_();
        saveUploadJob_(job);
        return publicUploadJob_(job);
      }

      fail_('YouTube chunk upload failed.', 'YOUTUBE_UPLOAD_ERROR', {
        status: code,
        body: uploadResponse.getContentText().slice(0, 4000),
        offset: job.offset,
        end: end
      });
    }

    if (job.offset >= job.totalBytes && job.youtubeVideo && job.youtubeVideo.id) {
      job = finalizeUploadJob_(job);
    } else {
      job.status = 'queued';
      job.updatedAt = isoNow_();
      saveUploadJob_(job);
    }
    return publicUploadJob_(job);
  } catch (error) {
    const failedJob = getUploadJob_(jobId);
    if (failedJob) {
      failedJob.status = 'error';
      failedJob.lastError = error.message;
      failedJob.updatedAt = isoNow_();
      saveUploadJob_(failedJob);
      writeAuditLog_('youtube.upload.error', {
        jobId: jobId,
        error: serializeError_(error)
      });
    }
    throw error;
  } finally {
    lock.releaseLock();
  }
}

function syncResumableUploadState_(job) {
  const response = UrlFetchApp.fetch(job.uploadUrl, {
    method: 'put',
    contentType: job.backupFile.mimeType,
    headers: {
      Authorization: 'Bearer ' + getOAuthToken_(),
      'Content-Range': 'bytes */' + job.totalBytes
    },
    muteHttpExceptions: true,
    followRedirects: false
  });
  const code = response.getResponseCode();

  if (code === 200 || code === 201) {
    job.offset = job.totalBytes;
    job.youtubeVideo = JSON.parse(response.getContentText() || '{}');
    job.status = 'finalizing';
    job.updatedAt = isoNow_();
    saveUploadJob_(job);
    return job;
  }

  if (code === 308) {
    const range = findHeader_(response.getHeaders(), 'Range');
    const match = String(range || '').match(/bytes=0-(\d+)/);
    job.offset = match ? Number(match[1]) + 1 : 0;
    job.updatedAt = isoNow_();
    saveUploadJob_(job);
    return job;
  }

  if (code === 404) {
    job.uploadUrl = initiateResumableUpload_(job.backupFile, buildVideoResource_(job.metadata));
    job.offset = 0;
    job.lastError = 'Resumable session expired and was restarted from the Drive backup.';
    job.updatedAt = isoNow_();
    saveUploadJob_(job);
    return job;
  }

  if (code >= 500 && code <= 599) {
    job.lastError = 'YouTube upload-status check returned HTTP ' + code + '; the next trigger will retry.';
    job.updatedAt = isoNow_();
    saveUploadJob_(job);
    return job;
  }

  fail_('Unable to reconcile the YouTube resumable upload session.', 'YOUTUBE_UPLOAD_STATUS_ERROR', {
    status: code,
    body: response.getContentText().slice(0, 4000)
  });
}

function finalizeUploadJob_(job) {
  if (job.status === 'complete') return job;
  const videoId = job.youtubeVideo.id;
  const artifacts = job.artifacts || {};
  const warnings = Array.isArray(job.warnings) ? job.warnings.slice() : [];

  if (job.metadata.thumbnailDriveFileId && !artifacts.thumbnail) {
    try {
      artifacts.thumbnail = setVideoThumbnail_(videoId, job.metadata.thumbnailDriveFileId);
    } catch (error) {
      warnings.push('Thumbnail failed: ' + error.message);
    }
  }

  if (job.metadata.playlistId && !artifacts.playlistItem) {
    try {
      artifacts.playlistItem = addVideoToPlaylist_({
        playlistId: job.metadata.playlistId,
        videoId: videoId
      });
    } catch (error) {
      warnings.push('Playlist insertion failed: ' + error.message);
    }
  }

  const completedAt = job.completedAt || isoNow_();
  const finalRecord = {
    jobId: job.id,
    completedAt: completedAt,
    youtubeVideoId: videoId,
    youtubeUrl: 'https://www.youtube.com/watch?v=' + videoId,
    privacyStatus: 'private',
    sourceFile: job.sourceFile,
    backupFile: job.backupFile,
    metadata: job.metadata,
    youtubeResponse: job.youtubeVideo,
    warnings: warnings
  };
  if (!artifacts.completionManifest) {
    try {
      artifacts.completionManifest = writeJsonFile_(
        'metadata',
        sanitizeFileName_(job.metadata.title) + '-' + videoId + '-complete.json',
        finalRecord
      );
    } catch (error) {
      warnings.push('Completion manifest failed: ' + error.message);
    }
  }

  job.artifacts = artifacts;
  job.status = 'complete';
  job.completedAt = completedAt;
  job.updatedAt = completedAt;
  job.warnings = warnings;
  saveUploadJob_(job);
  writeAuditLog_('youtube.upload.complete', finalRecord);
  if (!artifacts.completionEmail) {
    try {
      const email = sendUploadCompletionEmail_(job);
      artifacts.completionEmail = email ? { id: email.id || null, sentAt: isoNow_() } : { skipped: true };
    } catch (error) {
      warnings.push('Completion email failed: ' + error.message);
    }
    job.artifacts = artifacts;
    job.warnings = warnings;
    saveUploadJob_(job);
  }
  return job;
}

function processAllPendingUploads() {
  const jobs = listRawUploadJobs_().filter(function(job) {
    return job.status === 'queued' || job.status === 'uploading' || job.status === 'error';
  });
  const results = [];
  jobs.slice(0, 5).forEach(function(job) {
    const result = withResult_(function() {
      return processUploadJob_(job.id, APP.MAX_CHUNKS_PER_RUN);
    });
    results.push({ jobId: job.id, result: result });
  });
  return { processed: results.length, results: results, timestamp: isoNow_() };
}

function saveUploadJob_(job) {
  getScriptProperties_().setProperty(APP.JOB_PREFIX + job.id, JSON.stringify(job));
}

function getUploadJob_(jobId) {
  const raw = getScriptProperties_().getProperty(APP.JOB_PREFIX + jobId);
  return raw ? JSON.parse(raw) : null;
}

function listRawUploadJobs_() {
  const properties = getScriptProperties_().getProperties();
  return Object.keys(properties)
    .filter(function(key) { return key.indexOf(APP.JOB_PREFIX) === 0; })
    .map(function(key) {
      try { return JSON.parse(properties[key]); } catch (error) { return null; }
    })
    .filter(Boolean)
    .sort(function(a, b) { return String(b.createdAt).localeCompare(String(a.createdAt)); });
}

function listUploadJobs_() {
  return listRawUploadJobs_().slice(0, 50).map(publicUploadJob_);
}

function publicUploadJob_(job) {
  if (!job) return null;
  const result = JSON.parse(JSON.stringify(job));
  delete result.uploadUrl;
  result.progressPercent = result.totalBytes
    ? Math.min(100, Math.round((Number(result.offset || 0) / Number(result.totalBytes)) * 10000) / 100)
    : 0;
  result.totalBytesHuman = bytesToHuman_(result.totalBytes);
  result.offsetHuman = bytesToHuman_(result.offset);
  return result;
}
