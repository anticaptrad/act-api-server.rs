const HTTP_ACTION_POLICY = Object.freeze({
  health: 'public',
  channel: 'read',
  videos: 'read',
  analytics: 'read',
  jobs: 'read',
  partnerStatus: 'read',
  partnerOwners: 'read',
  partnerClaims: 'read',
  adminStatus: 'read',
  workspaceUsers: 'read',
  exportAnalytics: 'mutate',
  ingestVideo: 'mutate',
  startUpload: 'mutate',
  processUpload: 'mutate',
  processAllUploads: 'mutate',
  publishVideo: 'mutate',
  updateVideo: 'mutate',
  createPlaylist: 'mutate',
  addToPlaylist: 'mutate',
  ingestGmail: 'mutate',
  sendDigest: 'mutate'
});

const UI_ONLY_ACTIONS = Object.freeze({
  bootstrap: true,
  setup: true,
  rotateApiKey: true,
  saveConfig: true
});

function doGet(e) {
  const action = e && e.parameter ? e.parameter.action : '';
  if (action) {
    return jsonOutput_(handleHttpRequest_('GET', action, e.parameter || {}));
  }
  if (!isFirstPartyUiCaller_()) {
    return HtmlService.createHtmlOutput(
      '<!doctype html><meta charset="utf-8"><title>Anticaptrad API</title>' +
      '<h1>Anticaptrad YouTube API</h1><p>The browser dashboard is owner-only. Use an API-key-protected JSON POST request for automation.</p>'
    ).setTitle(APP.NAME + ' API');
  }
  return HtmlService
    .createTemplateFromFile('Index')
    .evaluate()
    .setTitle(APP.NAME)
    .setXFrameOptionsMode(HtmlService.XFrameOptionsMode.DEFAULT);
}

function doPost(e) {
  const body = parseJsonBody_(e);
  return jsonOutput_(handleHttpRequest_('POST', body.action || '', body));
}

function handleHttpRequest_(method, action, payload) {
  return withResult_(function() {
    action = requiredString_(action, 'action', 80);
    const policy = HTTP_ACTION_POLICY[action];
    if (!policy) {
      fail_('Action is not available through the public HTTP surface.', 'ACTION_NOT_AVAILABLE');
    }
    if (method === 'GET' && policy !== 'public') {
      fail_('Only the health action supports GET. Send API keys and privileged actions in a JSON POST body.', 'METHOD_NOT_ALLOWED');
    }
    if (policy !== 'public') assertApiKey_(payload && payload.apiKey);
    if (policy === 'mutate') {
      requiredString_(payload && payload.controlRequestId, 'controlRequestId', 200);
    }
    return dispatchAction_(action, payload || {}, {
      transport: 'http',
      method: method,
      policy: policy
    });
  });
}

function isFirstPartyUiCaller_() {
  // Do not trust the source profile alone: deployment settings can drift after a
  // clasp push. Anonymous/public executions have no active-user identity, while
  // the effective user is the deploying account. Require both identities to be
  // present and equal before rendering HTML or accepting google.script.run RPC.
  if (DEPLOYMENT_PROFILE.PUBLIC_HTTP) return false;
  try {
    const active = String(Session.getActiveUser().getEmail() || '').trim().toLowerCase();
    const effective = String(Session.getEffectiveUser().getEmail() || '').trim().toLowerCase();
    return Boolean(active && effective && active === effective);
  } catch (error) {
    console.warn('Unable to verify first-party UI caller: ' + error.message);
    return false;
  }
}

/** Called by the first-party HTML UI through google.script.run. */
function rpc(action, payload) {
  return withResult_(function() {
    if (!isFirstPartyUiCaller_()) fail_('The browser dashboard is owner-only.', 'UNAUTHORIZED');
    return dispatchAction_(requiredString_(action, 'action', 80), payload || {}, {
      transport: 'ui',
      method: 'RPC',
      policy: 'owner'
    });
  });
}

function dispatchAction_(action, payload, context) {
  const actions = {
    health: function() { return getHealth_(); },
    bootstrap: function() { return getBootstrap_(); },
    setup: function() { return setupApplication_(payload); },
    channel: function() { return getChannelSummary_(); },
    videos: function() { return listRecentVideos_(payload); },
    analytics: function() { return getAnalyticsReport_(payload); },
    exportAnalytics: function() { return exportAnalyticsReport_(payload); },
    jobs: function() { return listUploadJobs_(); },
    ingestVideo: function() { return ingestHttpVideo_(payload); },
    startUpload: function() { return startUploadJob_(payload); },
    processUpload: function() { return processUploadJob_(payload.jobId, payload.maxChunks); },
    processAllUploads: function() { return processAllPendingUploads(); },
    publishVideo: function() { return publishVideo_(payload); },
    updateVideo: function() { return updateVideoMetadata_(payload); },
    createPlaylist: function() { return createPlaylist_(payload); },
    addToPlaylist: function() { return addVideoToPlaylist_(payload); },
    ingestGmail: function() { return ingestGmailVideoAttachments_(payload); },
    sendDigest: function() { return sendOperationsDigest_(payload); },
    rotateApiKey: function() { return rotateApiKey_(); },
    saveConfig: function() { return savePublicConfig_(payload); },
    partnerStatus: function() { return getPartnerStatus_(); },
    partnerOwners: function() { return listPartnerContentOwners_(); },
    partnerClaims: function() { return listPartnerClaims_(payload); },
    adminStatus: function() { return getAdminStatus_(); },
    workspaceUsers: function() { return listWorkspaceUsers_(payload.maxResults); }
  };
  if (!actions[action]) fail_('Unknown action: ' + action, 'UNKNOWN_ACTION');
  if (context && context.transport === 'http' && UI_ONLY_ACTIONS[action]) {
    fail_('This action is owner-dashboard only.', 'ACTION_NOT_AVAILABLE');
  }

  const controlRequestId = context && context.policy === 'mutate'
    ? requiredString_(payload.controlRequestId, 'controlRequestId', 200)
    : '';
  if (controlRequestId) {
    writeAuditLog_('http.control.requested', {
      action: action,
      controlRequestId: controlRequestId
    });
  }

  try {
    const result = actions[action]();
    if (controlRequestId) {
      writeAuditLog_('http.control.completed', {
        action: action,
        controlRequestId: controlRequestId
      });
    }
    return result;
  } catch (error) {
    if (controlRequestId) {
      writeAuditLog_('http.control.failed', {
        action: action,
        controlRequestId: controlRequestId,
        errorCode: error && error.code ? String(error.code) : 'UNEXPECTED_ERROR'
      });
    }
    throw error;
  }
}

function getHealth_() {
  return {
    app: APP.NAME,
    version: APP.VERSION,
    deploymentProfile: DEPLOYMENT_PROFILE.NAME,
    publicHttp: DEPLOYMENT_PROFILE.PUBLIC_HTTP,
    timestamp: isoNow_(),
    configured: Boolean(getConfigValue_(APP.CONFIG_KEYS.ROOT_FOLDER_ID, ''))
  };
}

function getBootstrap_() {
  const channelResult = withResult_(getChannelSummary_);
  return {
    health: getHealth_(),
    channel: channelResult.ok ? channelResult.data : null,
    channelError: channelResult.ok ? null : channelResult.error,
    config: getPublicConfig_(),
    jobs: listUploadJobs_(),
    partner: getPartnerStatus_(),
    admin: getAdminStatus_(),
    defaults: {
      analyticsStartDate: formatDate_(new Date(Date.now() - 28 * 24 * 60 * 60 * 1000)),
      analyticsEndDate: formatDate_(new Date()),
      metrics: ANALYTICS_METRICS,
      dimensions: ANALYTICS_DIMENSIONS
    }
  };
}

function getPublicConfig_() {
  return {
    expectedChannelHandle: getConfigValue_(
      APP.CONFIG_KEYS.EXPECTED_CHANNEL_HANDLE,
      APP.EXPECTED_CHANNEL_HANDLE
    ),
    expectedChannelId: getConfigValue_(APP.CONFIG_KEYS.EXPECTED_CHANNEL_ID, ''),
    rootFolderId: getConfigValue_(APP.CONFIG_KEYS.ROOT_FOLDER_ID, ''),
    allowPublicUploads: isTrue_(getConfigValue_(APP.CONFIG_KEYS.ALLOW_PUBLIC_UPLOADS, 'false')),
    notificationEmail: getConfigValue_(APP.CONFIG_KEYS.NOTIFICATION_EMAIL, getActiveEmail_()),
    apiKeyConfigured: Boolean(getConfigValue_(APP.CONFIG_KEYS.API_KEY_HASH, '')),
    apiKeyLast4: getConfigValue_(APP.CONFIG_KEYS.API_KEY_LAST4, ''),
    contentOwnerId: getConfigValue_(APP.CONFIG_KEYS.CONTENT_OWNER_ID, ''),
    workspaceCustomerId: getConfigValue_(APP.CONFIG_KEYS.WORKSPACE_CUSTOMER_ID, '')
  };
}

function savePublicConfig_(payload) {
  const values = {};
  if (payload.expectedChannelHandle !== undefined) {
    const handle = optionalString_(payload.expectedChannelHandle, 100);
    if (handle && handle.charAt(0) !== '@') {
      fail_('Expected channel handle must start with @.', 'VALIDATION_ERROR');
    }
    values[APP.CONFIG_KEYS.EXPECTED_CHANNEL_HANDLE] = handle;
  }
  if (payload.expectedChannelId !== undefined) {
    values[APP.CONFIG_KEYS.EXPECTED_CHANNEL_ID] = optionalString_(payload.expectedChannelId, 100);
  }
  if (payload.allowPublicUploads !== undefined) {
    values[APP.CONFIG_KEYS.ALLOW_PUBLIC_UPLOADS] = String(Boolean(payload.allowPublicUploads));
  }
  if (payload.notificationEmail !== undefined) {
    const email = optionalString_(payload.notificationEmail, 254);
    if (email && !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
      fail_('Notification email is invalid.', 'VALIDATION_ERROR');
    }
    values[APP.CONFIG_KEYS.NOTIFICATION_EMAIL] = email;
  }
  if (payload.contentOwnerId !== undefined) {
    values[APP.CONFIG_KEYS.CONTENT_OWNER_ID] = optionalString_(payload.contentOwnerId, 200);
  }
  if (payload.workspaceCustomerId !== undefined) {
    values[APP.CONFIG_KEYS.WORKSPACE_CUSTOMER_ID] = optionalString_(payload.workspaceCustomerId, 200);
  }
  setConfigValues_(values);
  return getPublicConfig_();
}

function rotateApiKey_() {
  const raw = 'acp_' + uuid_().replace(/-/g, '') + uuid_().replace(/-/g, '');
  setConfigValues_({
    API_KEY_HASH: sha256Hex_(raw),
    API_KEY_LAST4: raw.slice(-4)
  });
  return {
    apiKey: raw,
    last4: raw.slice(-4),
    warning: 'Copy this key now. Only its SHA-256 hash is stored.'
  };
}

function assertApiKey_(provided) {
  const expectedHash = getConfigValue_(APP.CONFIG_KEYS.API_KEY_HASH, '');
  if (!expectedHash) fail_('HTTP API key is not configured. Rotate one in the owner web UI.', 'API_KEY_NOT_CONFIGURED');
  const actualHash = sha256Hex_(requiredString_(provided, 'apiKey', 300));
  if (!constantTimeEquals_(actualHash, expectedHash)) fail_('Invalid API key.', 'UNAUTHORIZED');
}
