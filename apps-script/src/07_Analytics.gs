function getAnalyticsReport_(payload) {
  payload = payload || {};
  const channel = getMyChannel_();
  assertExpectedChannel_(channel, false);
  const startDate = assertDateString_(payload.startDate || formatDate_(new Date(Date.now() - 28 * 86400000)), 'startDate');
  const endDate = assertDateString_(payload.endDate || formatDate_(new Date()), 'endDate');
  if (startDate > endDate) fail_('startDate must not be after endDate.', 'VALIDATION_ERROR');

  const dimensions = normalizeAnalyticsValues_(payload.dimensions || ['day'], ANALYTICS_DIMENSIONS, 'dimensions');
  const metrics = normalizeAnalyticsValues_(
    payload.metrics || ['views', 'estimatedMinutesWatched', 'averageViewDuration', 'subscribersGained'],
    ANALYTICS_METRICS,
    'metrics'
  );
  const request = {
    ids: 'channel==' + channel.id,
    startDate: startDate,
    endDate: endDate,
    metrics: metrics.join(','),
    dimensions: dimensions.join(','),
    sort: dimensions.join(','),
    maxResults: Math.min(Math.max(Number(payload.maxResults || 200), 1), 200)
  };
  if (payload.filters) request.filters = optionalString_(payload.filters, 1000);
  const report = YouTubeAnalytics.Reports.query(request);
  return normalizeAnalyticsReport_(report, request);
}

function normalizeAnalyticsValues_(value, allowed, fieldName) {
  const values = Array.isArray(value) ? value : String(value || '').split(',');
  const clean = values.map(function(item) { return String(item).trim(); }).filter(Boolean);
  if (!clean.length) fail_(fieldName + ' cannot be empty.', 'VALIDATION_ERROR');
  clean.forEach(function(item) {
    if (allowed.indexOf(item) === -1) {
      fail_('Unsupported ' + fieldName + ' value: ' + item, 'VALIDATION_ERROR', { allowed: allowed });
    }
  });
  return clean;
}

function normalizeAnalyticsReport_(report, request) {
  const headers = (report.columnHeaders || []).map(function(header) {
    return {
      name: header.name,
      columnType: header.columnType,
      dataType: header.dataType
    };
  });
  return {
    request: request,
    headers: headers,
    rows: report.rows || [],
    rowCount: report.rows ? report.rows.length : 0,
    kind: report.kind || '',
    generatedAt: isoNow_()
  };
}

function exportAnalyticsReport_(payload) {
  const report = getAnalyticsReport_(payload);
  const headers = report.headers.map(function(header) { return header.name; });
  const lines = [headers.map(csvEscape_).join(',')];
  report.rows.forEach(function(row) {
    lines.push(row.map(csvEscape_).join(','));
  });
  const baseName = 'anticaptrad-youtube-analytics-' + report.request.startDate + '-to-' + report.request.endDate;
  const csv = writeTextFile_('reports', baseName + '.csv', 'text/csv', lines.join('\n'));
  const json = writeJsonFile_('reports', baseName + '.json', report);
  writeAuditLog_('youtube.analytics.exported', {
    startDate: report.request.startDate,
    endDate: report.request.endDate,
    rowCount: report.rowCount,
    csvFileId: csv.id,
    jsonFileId: json.id
  });
  return { report: report, csv: csv, json: json };
}
