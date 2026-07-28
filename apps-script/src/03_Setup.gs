function setupApplication_(payload) {
  payload = payload || {};
  const lock = LockService.getScriptLock();
  lock.waitLock(30000);
  try {
    const expectedHandle = optionalString_(
      payload.expectedChannelHandle || getConfigValue_(APP.CONFIG_KEYS.EXPECTED_CHANNEL_HANDLE, APP.EXPECTED_CHANNEL_HANDLE),
      100
    ) || APP.EXPECTED_CHANNEL_HANDLE;

    setConfigValues_({
      EXPECTED_CHANNEL_HANDLE: expectedHandle,
      NOTIFICATION_EMAIL: payload.notificationEmail || getActiveEmail_(),
      ALLOW_PUBLIC_UPLOADS: 'false'
    });

    const folders = ensureFolderStructure_();
    const channel = getMyChannel_();
    const channelCheck = assertExpectedChannel_(channel, true);
    setConfigValues_({
      EXPECTED_CHANNEL_ID: channel.id,
      ROOT_FOLDER_ID: folders.root.id
    });

    ensureUploadTrigger_();
    const apiKeyInfo = getConfigValue_(APP.CONFIG_KEYS.API_KEY_HASH, '') ? null : rotateApiKey_();
    writeAuditLog_('application.setup', {
      channelId: channel.id,
      channelTitle: channel.snippet && channel.snippet.title,
      channelHandle: channel.snippet && channel.snippet.customUrl,
      rootFolderId: folders.root.id,
      actor: getActiveEmail_()
    });

    return {
      channel: summarizeChannel_(channel),
      channelCheck: channelCheck,
      folders: folders,
      config: getPublicConfig_(),
      apiKey: apiKeyInfo,
      triggerInstalled: true,
      message: 'Setup complete. Uploads remain private and public publishing remains disabled.'
    };
  } finally {
    lock.releaseLock();
  }
}

function ensureUploadTrigger_() {
  const functionName = 'processAllPendingUploads';
  const exists = ScriptApp.getProjectTriggers().some(function(trigger) {
    return trigger.getHandlerFunction() === functionName;
  });
  if (!exists) {
    ScriptApp.newTrigger(functionName).timeBased().everyMinutes(5).create();
  }
}
