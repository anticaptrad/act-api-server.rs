function getMyChannel_() {
  const response = YouTube.Channels.list(
    'id,snippet,contentDetails,statistics,status,brandingSettings',
    { mine: true, maxResults: 1 }
  );
  if (!response || !response.items || !response.items.length) {
    fail_('No YouTube channel is associated with the authorized Google account.', 'CHANNEL_NOT_FOUND');
  }
  return response.items[0];
}

function normalizeHandle_(value) {
  return String(value || '').trim().toLowerCase().replace(/^https?:\/\/(www\.)?youtube\.com\//, '');
}

function assertExpectedChannel_(channel, allowInitialIdCapture) {
  const expectedHandle = normalizeHandle_(getConfigValue_(
    APP.CONFIG_KEYS.EXPECTED_CHANNEL_HANDLE,
    APP.EXPECTED_CHANNEL_HANDLE
  ));
  const expectedId = getConfigValue_(APP.CONFIG_KEYS.EXPECTED_CHANNEL_ID, '');
  const actualHandle = normalizeHandle_(channel && channel.snippet && channel.snippet.customUrl);
  const actualId = channel && channel.id ? String(channel.id) : '';
  const warnings = [];

  if (expectedId && actualId !== expectedId) {
    fail_('Authorized channel ID does not match the configured Anticaptrad channel.', 'WRONG_CHANNEL', {
      expectedChannelId: expectedId,
      actualChannelId: actualId,
      actualHandle: actualHandle
    });
  }

  if (expectedHandle && actualHandle && expectedHandle !== actualHandle) {
    fail_('Authorized channel handle does not match the configured Anticaptrad handle.', 'WRONG_CHANNEL', {
      expectedHandle: expectedHandle,
      actualHandle: actualHandle,
      actualChannelId: actualId
    });
  }

  if (expectedHandle && !actualHandle) {
    warnings.push('YouTube did not return a custom handle; channel ID verification is being used instead.');
  }

  if (!expectedId && allowInitialIdCapture && actualId) {
    setConfigValues_({ EXPECTED_CHANNEL_ID: actualId });
  }

  return {
    verified: true,
    expectedHandle: expectedHandle,
    actualHandle: actualHandle,
    channelId: actualId,
    warnings: warnings
  };
}

function summarizeChannel_(channel) {
  const snippet = channel.snippet || {};
  const stats = channel.statistics || {};
  const status = channel.status || {};
  const thumbnails = snippet.thumbnails || {};
  const image = thumbnails.high || thumbnails.medium || thumbnails.default || {};
  return {
    id: channel.id,
    title: snippet.title || '',
    handle: snippet.customUrl || '',
    description: snippet.description || '',
    publishedAt: snippet.publishedAt || null,
    country: snippet.country || '',
    thumbnailUrl: image.url || '',
    uploadsPlaylistId: channel.contentDetails && channel.contentDetails.relatedPlaylists
      ? channel.contentDetails.relatedPlaylists.uploads
      : '',
    statistics: {
      viewCount: Number(stats.viewCount || 0),
      subscriberCount: Number(stats.subscriberCount || 0),
      videoCount: Number(stats.videoCount || 0),
      hiddenSubscriberCount: Boolean(stats.hiddenSubscriberCount)
    },
    status: {
      privacyStatus: status.privacyStatus || '',
      isLinked: Boolean(status.isLinked),
      longUploadsStatus: status.longUploadsStatus || '',
      madeForKids: status.madeForKids,
      selfDeclaredMadeForKids: status.selfDeclaredMadeForKids
    }
  };
}

function getChannelSummary_() {
  const channel = getMyChannel_();
  const verification = assertExpectedChannel_(channel, false);
  return {
    channel: summarizeChannel_(channel),
    verification: verification
  };
}

function listRecentVideos_(payload) {
  payload = payload || {};
  const maxResults = Math.min(Math.max(Number(payload.maxResults || 25), 1), 50);
  const channel = getMyChannel_();
  assertExpectedChannel_(channel, false);
  const playlistId = channel.contentDetails.relatedPlaylists.uploads;
  const playlistItems = YouTube.PlaylistItems.list('snippet,contentDetails,status', {
    playlistId: playlistId,
    maxResults: maxResults
  });
  const items = playlistItems && playlistItems.items ? playlistItems.items : [];
  const videoIds = items.map(function(item) {
    return item.contentDetails && item.contentDetails.videoId;
  }).filter(Boolean);
  let detailsById = {};
  if (videoIds.length) {
    const details = YouTube.Videos.list('snippet,status,contentDetails,statistics', {
      id: videoIds.join(','),
      maxResults: 50
    });
    (details.items || []).forEach(function(video) { detailsById[video.id] = video; });
  }
  return {
    items: items.map(function(item) {
      const id = item.contentDetails.videoId;
      const video = detailsById[id] || {};
      const snippet = video.snippet || item.snippet || {};
      const status = video.status || {};
      const stats = video.statistics || {};
      const thumb = snippet.thumbnails && (snippet.thumbnails.medium || snippet.thumbnails.default);
      return {
        id: id,
        title: snippet.title || '',
        description: snippet.description || '',
        publishedAt: snippet.publishedAt || item.contentDetails.videoPublishedAt || null,
        privacyStatus: status.privacyStatus || item.status && item.status.privacyStatus || '',
        uploadStatus: status.uploadStatus || '',
        embeddable: status.embeddable,
        madeForKids: status.madeForKids,
        thumbnailUrl: thumb ? thumb.url : '',
        duration: video.contentDetails && video.contentDetails.duration || '',
        statistics: {
          viewCount: Number(stats.viewCount || 0),
          likeCount: Number(stats.likeCount || 0),
          commentCount: Number(stats.commentCount || 0)
        },
        youtubeUrl: 'https://www.youtube.com/watch?v=' + encodeURIComponent(id)
      };
    }),
    nextPageToken: playlistItems && playlistItems.nextPageToken || null
  };
}


function mutableVideoSnippet_(snippet) {
  snippet = snippet || {};
  const result = {
    title: snippet.title || '',
    description: snippet.description || '',
    categoryId: snippet.categoryId || '22'
  };
  if (snippet.tags) result.tags = snippet.tags;
  if (snippet.defaultLanguage) result.defaultLanguage = snippet.defaultLanguage;
  if (snippet.defaultAudioLanguage) result.defaultAudioLanguage = snippet.defaultAudioLanguage;
  return result;
}

function mutableVideoStatus_(status) {
  status = status || {};
  const result = {
    privacyStatus: status.privacyStatus || 'private'
  };
  if (status.embeddable !== undefined) result.embeddable = status.embeddable;
  if (status.license) result.license = status.license;
  if (status.publicStatsViewable !== undefined) result.publicStatsViewable = status.publicStatsViewable;
  if (status.selfDeclaredMadeForKids !== undefined) result.selfDeclaredMadeForKids = status.selfDeclaredMadeForKids;
  if (status.publishAt) result.publishAt = status.publishAt;
  return result;
}

function updateVideoMetadata_(payload) {
  const videoId = requiredString_(payload.videoId, 'videoId', 100);
  const existing = YouTube.Videos.list('snippet,status', { id: videoId, maxResults: 1 });
  if (!existing.items || !existing.items.length) fail_('Video not found.', 'VIDEO_NOT_FOUND');
  const video = existing.items[0];
  const channel = getMyChannel_();
  assertExpectedChannel_(channel, false);
  if (video.snippet.channelId !== channel.id) fail_('Video is not owned by the configured channel.', 'WRONG_CHANNEL');

  const snippet = mutableVideoSnippet_(video.snippet);
  if (payload.title !== undefined) snippet.title = requiredString_(payload.title, 'title', 100);
  if (payload.description !== undefined) snippet.description = optionalString_(payload.description, 5000);
  if (payload.tags !== undefined) snippet.tags = normalizeTags_(payload.tags);
  if (payload.categoryId !== undefined) snippet.categoryId = requiredString_(payload.categoryId, 'categoryId', 10);
  if (payload.defaultLanguage !== undefined) snippet.defaultLanguage = optionalString_(payload.defaultLanguage, 20) || undefined;

  const resource = { id: videoId, snippet: snippet };
  let parts = 'snippet';
  if (payload.madeForKids !== undefined) {
    resource.status = mutableVideoStatus_(video.status);
    resource.status.selfDeclaredMadeForKids = Boolean(payload.madeForKids);
    parts += ',status';
  }
  const updated = YouTube.Videos.update(resource, parts);
  writeAuditLog_('youtube.video.metadata_updated', { videoId: videoId, fields: Object.keys(payload) });
  return updated;
}

function publishVideo_(payload) {
  const videoId = requiredString_(payload.videoId, 'videoId', 100);
  const privacyStatus = requiredString_(payload.privacyStatus, 'privacyStatus', 20).toLowerCase();
  if (VIDEO_PRIVACY_VALUES.indexOf(privacyStatus) === -1) {
    fail_('privacyStatus must be private, unlisted, or public.', 'VALIDATION_ERROR');
  }
  if (privacyStatus !== 'private' && !isTrue_(getConfigValue_(APP.CONFIG_KEYS.ALLOW_PUBLIC_UPLOADS, 'false'))) {
    fail_('Public and unlisted publishing is disabled in configuration.', 'PUBLICATION_DISABLED');
  }
  const expectedConfirmation = 'PUBLISH ' + videoId + ' AS ' + privacyStatus.toUpperCase();
  if (String(payload.confirmation || '') !== expectedConfirmation) {
    fail_('Exact confirmation phrase required: ' + expectedConfirmation, 'CONFIRMATION_REQUIRED');
  }

  const result = YouTube.Videos.list('snippet,status', { id: videoId, maxResults: 1 });
  if (!result.items || !result.items.length) fail_('Video not found.', 'VIDEO_NOT_FOUND');
  const video = result.items[0];
  const channel = getMyChannel_();
  assertExpectedChannel_(channel, false);
  if (video.snippet.channelId !== channel.id) fail_('Video is not owned by the configured channel.', 'WRONG_CHANNEL');
  const status = mutableVideoStatus_(video.status);
  status.privacyStatus = privacyStatus;
  if (payload.publishAt) {
    if (privacyStatus !== 'private') fail_('Scheduled publication requires privacyStatus=private.', 'VALIDATION_ERROR');
    const scheduled = new Date(payload.publishAt);
    if (isNaN(scheduled.getTime()) || scheduled.getTime() <= Date.now()) {
      fail_('publishAt must be a valid future date-time.', 'VALIDATION_ERROR');
    }
    status.publishAt = scheduled.toISOString();
  } else {
    delete status.publishAt;
  }
  const updated = YouTube.Videos.update({ id: videoId, status: status }, 'status');
  writeAuditLog_('youtube.video.privacy_changed', {
    videoId: videoId,
    privacyStatus: privacyStatus,
    publishAt: payload.publishAt || null
  });
  return updated;
}

function createPlaylist_(payload) {
  const title = requiredString_(payload.title, 'title', 150);
  const privacyStatus = requiredString_(payload.privacyStatus || 'private', 'privacyStatus', 20).toLowerCase();
  if (VIDEO_PRIVACY_VALUES.indexOf(privacyStatus) === -1) fail_('Invalid playlist privacy status.', 'VALIDATION_ERROR');
  if (privacyStatus !== 'private' && !isTrue_(getConfigValue_(APP.CONFIG_KEYS.ALLOW_PUBLIC_UPLOADS, 'false'))) {
    fail_('Non-private playlists are disabled in configuration.', 'PUBLICATION_DISABLED');
  }
  const playlist = YouTube.Playlists.insert({
    snippet: {
      title: title,
      description: optionalString_(payload.description, 5000),
      defaultLanguage: optionalString_(payload.defaultLanguage, 20) || undefined
    },
    status: { privacyStatus: privacyStatus }
  }, 'snippet,status');
  writeAuditLog_('youtube.playlist.created', { playlistId: playlist.id, title: title, privacyStatus: privacyStatus });
  return playlist;
}

function addVideoToPlaylist_(payload) {
  const playlistId = requiredString_(payload.playlistId, 'playlistId', 200);
  const videoId = requiredString_(payload.videoId, 'videoId', 100);
  const item = YouTube.PlaylistItems.insert({
    snippet: {
      playlistId: playlistId,
      resourceId: { kind: 'youtube#video', videoId: videoId },
      position: payload.position === undefined ? undefined : Number(payload.position)
    }
  }, 'snippet');
  writeAuditLog_('youtube.playlist.video_added', { playlistId: playlistId, videoId: videoId });
  return item;
}

function setVideoThumbnail_(videoId, driveFileId) {
  videoId = requiredString_(videoId, 'videoId', 100);
  driveFileId = requiredString_(driveFileId, 'thumbnailDriveFileId', 300);
  const blob = DriveApp.getFileById(driveFileId).getBlob();
  if (!/^image\/(jpeg|png)$/.test(blob.getContentType())) {
    fail_('Thumbnail must be image/jpeg or image/png.', 'INVALID_THUMBNAIL');
  }
  const result = YouTube.Thumbnails.set(videoId, blob);
  writeAuditLog_('youtube.video.thumbnail_set', { videoId: videoId, driveFileId: driveFileId });
  return result;
}
