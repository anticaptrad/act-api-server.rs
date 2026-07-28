function getPartnerStatus_() {
  const enabled = typeof YouTubeContentId !== 'undefined';
  const contentOwnerId = getConfigValue_(APP.CONFIG_KEYS.CONTENT_OWNER_ID, '');
  return {
    advancedServiceEnabled: enabled,
    contentOwnerIdConfigured: Boolean(contentOwnerId),
    available: enabled && Boolean(contentOwnerId),
    note: enabled
      ? (contentOwnerId ? 'Content ID service is enabled for the configured content owner.' : 'Set CONTENT_OWNER_ID before using partner operations.')
      : 'Optional only. Enable the partner manifest profile if this account has YouTube Content ID partner access.'
  };
}

function getAdminStatus_() {
  const enabled = typeof AdminDirectory !== 'undefined';
  const customerId = getConfigValue_(APP.CONFIG_KEYS.WORKSPACE_CUSTOMER_ID, '');
  return {
    advancedServiceEnabled: enabled,
    customerIdConfigured: Boolean(customerId),
    available: enabled && Boolean(customerId),
    note: enabled
      ? (customerId ? 'Admin Directory service is enabled.' : 'Set WORKSPACE_CUSTOMER_ID or use my_customer.')
      : 'Optional only. Admin SDK applies to Google Workspace administrators, not a normal @gmail.com account.'
  };
}

function listWorkspaceUsers_(maxResults) {
  if (typeof AdminDirectory === 'undefined') {
    fail_('Admin Directory advanced service is not enabled.', 'ADMIN_SDK_DISABLED');
  }
  const customer = getConfigValue_(APP.CONFIG_KEYS.WORKSPACE_CUSTOMER_ID, 'my_customer');
  return AdminDirectory.Users.list({
    customer: customer,
    maxResults: Math.min(Math.max(Number(maxResults || 50), 1), 200),
    orderBy: 'email',
    projection: 'basic'
  });
}

function listPartnerContentOwners_() {
  if (typeof YouTubeContentId === 'undefined') {
    fail_('YouTube Content ID advanced service is not enabled.', 'PARTNER_API_DISABLED');
  }
  return YouTubeContentId.ContentOwners.list({ fetchMine: true });
}

function listPartnerClaims_(payload) {
  if (typeof YouTubeContentId === 'undefined') {
    fail_('YouTube Content ID advanced service is not enabled.', 'PARTNER_API_DISABLED');
  }
  payload = payload || {};
  const contentOwnerId = requiredString_(
    getConfigValue_(APP.CONFIG_KEYS.CONTENT_OWNER_ID, ''),
    'CONTENT_OWNER_ID',
    200
  );
  const options = { onBehalfOfContentOwner: contentOwnerId };
  if (payload.id) options.id = optionalString_(payload.id, 2000);
  if (payload.videoId) options.videoId = optionalString_(payload.videoId, 2000);
  if (payload.assetId) options.assetId = optionalString_(payload.assetId, 500);
  if (payload.q) options.q = optionalString_(payload.q, 500);
  if (payload.pageToken) options.pageToken = optionalString_(payload.pageToken, 1000);
  return YouTubeContentId.Claims.list(options);
}
