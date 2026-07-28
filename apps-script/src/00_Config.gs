/**
 * Anticaptrad YouTube Control Center configuration.
 *
 * Safe defaults:
 * - Only the deploying user may open the web app.
 * - Uploads begin as private.
 * - Public/unlisted transitions are disabled until explicitly enabled.
 * - The authenticated YouTube channel must match @anticaptrad unless overridden.
 */
const DEPLOYMENT_PROFILE = Object.freeze({
  NAME: 'default',
  PUBLIC_HTTP: false
});

const APP = Object.freeze({
  NAME: 'Anticaptrad YouTube Control Center',
  VERSION: '1.0.3',
  EXPECTED_CHANNEL_HANDLE: '@anticaptrad',
  ROOT_FOLDER_NAME: 'Anticaptrad YouTube',
  FOLDERS: Object.freeze({
    INBOX: '01 Inbox',
    BACKUPS: '02 Source Backups',
    THUMBNAILS: '03 Thumbnails',
    METADATA: '04 Metadata',
    REPORTS: '05 Analytics Reports',
    LOGS: '06 Audit Logs'
  }),
  UPLOAD_CHUNK_BYTES: 8 * 1024 * 1024,
  MAX_CHUNKS_PER_RUN: 6,
  MAX_GMAIL_ATTACHMENT_BYTES: 20 * 1024 * 1024,
  JOB_PREFIX: 'UPLOAD_JOB_',
  IDEMPOTENCY_PREFIX: 'UPLOAD_IDEMPOTENCY_',
  PROCESSED_GMAIL_PREFIX: 'GMAIL_DONE_',
  CONFIG_KEYS: Object.freeze({
    ROOT_FOLDER_ID: 'ROOT_FOLDER_ID',
    EXPECTED_CHANNEL_HANDLE: 'EXPECTED_CHANNEL_HANDLE',
    EXPECTED_CHANNEL_ID: 'EXPECTED_CHANNEL_ID',
    ALLOW_PUBLIC_UPLOADS: 'ALLOW_PUBLIC_UPLOADS',
    NOTIFICATION_EMAIL: 'NOTIFICATION_EMAIL',
    API_KEY_HASH: 'API_KEY_HASH',
    API_KEY_LAST4: 'API_KEY_LAST4',
    CONTENT_OWNER_ID: 'CONTENT_OWNER_ID',
    WORKSPACE_CUSTOMER_ID: 'WORKSPACE_CUSTOMER_ID'
  })
});

const ANALYTICS_METRICS = Object.freeze([
  'views',
  'engagedViews',
  'estimatedMinutesWatched',
  'averageViewDuration',
  'averageViewPercentage',
  'likes',
  'comments',
  'shares',
  'subscribersGained',
  'subscribersLost'
]);

const ANALYTICS_DIMENSIONS = Object.freeze([
  'day',
  'video',
  'country',
  'deviceType',
  'operatingSystem',
  'trafficSourceType'
]);

const VIDEO_PRIVACY_VALUES = Object.freeze(['private', 'unlisted', 'public']);
