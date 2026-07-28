function ingestGmailVideoAttachments_(payload) {
  payload = payload || {};
  const query = optionalString_(payload.query || 'label:anticaptrad-video-inbox has:attachment', 500);
  const maxMessages = Math.min(Math.max(Number(payload.maxMessages || 10), 1), 25);
  const response = Gmail.Users.Messages.list('me', { q: query, maxResults: maxMessages });
  const messages = response.messages || [];
  const inboxFolder = getFolderByLogicalName_('inbox');
  const imported = [];
  const skipped = [];

  messages.forEach(function(ref) {
    const message = Gmail.Users.Messages.get('me', ref.id, { format: 'full' });
    const attachments = [];
    collectGmailAttachments_(message.payload, attachments);
    attachments.forEach(function(part) {
      const marker = APP.PROCESSED_GMAIL_PREFIX + message.id + '_' + part.body.attachmentId;
      if (getScriptProperties_().getProperty(marker)) {
        skipped.push({ filename: part.filename, reason: 'already imported' });
        return;
      }
      const mimeType = String(part.mimeType || '');
      if (!/^video\//.test(mimeType)) {
        skipped.push({ filename: part.filename, reason: 'not a video attachment' });
        return;
      }
      const size = Number(part.body.size || 0);
      if (size > APP.MAX_GMAIL_ATTACHMENT_BYTES) {
        skipped.push({ filename: part.filename, reason: 'attachment exceeds ' + bytesToHuman_(APP.MAX_GMAIL_ATTACHMENT_BYTES) });
        return;
      }
      const attachment = Gmail.Users.Messages.Attachments.get('me', message.id, part.body.attachmentId);
      const bytes = Utilities.base64DecodeWebSafe(attachment.data);
      const filename = sanitizeFileName_(part.filename || ('gmail-video-' + message.id + '.bin'));
      const file = inboxFolder.createFile(Utilities.newBlob(bytes, mimeType, filename));
      getScriptProperties_().setProperty(marker, isoNow_());
      imported.push({
        messageId: message.id,
        filename: filename,
        mimeType: mimeType,
        size: bytes.length,
        sizeHuman: bytesToHuman_(bytes.length),
        driveFileId: file.getId(),
        driveUrl: file.getUrl()
      });
    });
  });

  writeAuditLog_('gmail.video_attachments.ingested', {
    query: query,
    imported: imported,
    skipped: skipped
  });
  return { imported: imported, skipped: skipped, messageCount: messages.length };
}

function collectGmailAttachments_(part, output) {
  if (!part) return;
  if (part.filename && part.body && part.body.attachmentId) output.push(part);
  (part.parts || []).forEach(function(child) { collectGmailAttachments_(child, output); });
}

function sendGmailMessage_(to, subject, bodyText) {
  to = requiredString_(to, 'to', 254);
  subject = requiredString_(subject, 'subject', 998);
  const rawMessage = [
    'To: ' + to,
    'Subject: ' + subject,
    'MIME-Version: 1.0',
    'Content-Type: text/plain; charset=UTF-8',
    'Content-Transfer-Encoding: 8bit',
    '',
    String(bodyText || '')
  ].join('\r\n');
  const encoded = Utilities.base64EncodeWebSafe(rawMessage, Utilities.Charset.UTF_8).replace(/=+$/g, '');
  return Gmail.Users.Messages.send({ raw: encoded }, 'me');
}

function sendUploadCompletionEmail_(job) {
  const to = getConfigValue_(APP.CONFIG_KEYS.NOTIFICATION_EMAIL, getActiveEmail_());
  if (!to) return null;
  const videoId = job.youtubeVideo && job.youtubeVideo.id;
  const lines = [
    'Anticaptrad YouTube upload completed.',
    '',
    'Title: ' + job.metadata.title,
    'Video ID: ' + videoId,
    'YouTube URL: https://www.youtube.com/watch?v=' + videoId,
    'Privacy: private',
    'Backup: ' + (job.backupFile.webViewLink || job.backupFile.id),
    'Completed: ' + job.completedAt,
    '',
    'The project intentionally leaves the video private. Use the web app and exact confirmation phrase to publish it.'
  ];
  return sendGmailMessage_(to, '[Anticaptrad] Private YouTube upload complete: ' + job.metadata.title, lines.join('\n'));
}

function sendOperationsDigest_() {
  const to = getConfigValue_(APP.CONFIG_KEYS.NOTIFICATION_EMAIL, getActiveEmail_());
  if (!to) fail_('Notification email is not configured.', 'CONFIG_ERROR');
  const channel = getChannelSummary_();
  const jobs = listUploadJobs_().slice(0, 10);
  const lines = [
    'Anticaptrad YouTube operations digest',
    'Generated: ' + isoNow_(),
    '',
    'Channel: ' + channel.channel.title + ' ' + channel.channel.handle,
    'Subscribers: ' + channel.channel.statistics.subscriberCount,
    'Videos: ' + channel.channel.statistics.videoCount,
    'Views: ' + channel.channel.statistics.viewCount,
    '',
    'Recent upload jobs:'
  ];
  jobs.forEach(function(job) {
    lines.push('- ' + job.status + ' | ' + job.progressPercent + '% | ' + job.metadata.title + ' | ' + (job.youtubeVideo && job.youtubeVideo.id || job.id));
  });
  const result = sendGmailMessage_(to, '[Anticaptrad] YouTube operations digest', lines.join('\n'));
  writeAuditLog_('gmail.operations_digest.sent', { to: to, jobCount: jobs.length });
  return { sent: true, to: to, gmailMessageId: result.id || null };
}
