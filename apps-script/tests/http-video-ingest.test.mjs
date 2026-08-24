import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function mp4Bytes(label) {
  const bytes = Buffer.alloc(64);
  bytes.writeUInt32BE(bytes.length, 0);
  bytes.write('ftyp', 4, 'ascii');
  bytes.write('isom', 8, 'ascii');
  bytes.write(String(label), 16, 'utf8');
  return bytes;
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function createHarness() {
  const files = new Map();
  const scriptProperties = new Map();
  const audits = [];
  let nextId = 1;
  let lockHeld = false;

  const folder = {
    getId: () => 'inbox-folder',
    getFilesByName(name) {
      const matches = [...files.values()].filter((file) => file.name === name);
      let index = 0;
      return {
        hasNext: () => index < matches.length,
        next: () => {
          const file = matches[index];
          index += 1;
          return { getId: () => file.id };
        },
      };
    },
  };

  const context = vm.createContext({
    console: { error() {}, warn() {}, log() {} },
    Session: { getScriptTimeZone: () => 'UTC' },
    LockService: {
      getScriptLock: () => ({
        tryLock() {
          if (lockHeld) return false;
          lockHeld = true;
          return true;
        },
        releaseLock() {
          lockHeld = false;
        },
      }),
    },
    PropertiesService: {
      getScriptProperties: () => ({
        getProperty: (key) => scriptProperties.get(key) ?? null,
        setProperty: (key, value) => scriptProperties.set(key, value),
      }),
    },
    DriveApp: { getFolderById: () => folder },
    Drive: {
      Files: {
        get(id) {
          const file = files.get(id);
          if (!file) throw new Error('missing fixture file');
          return structuredClone(file);
        },
        create(resource, blob) {
          const id = `file-${nextId}`;
          nextId += 1;
          const file = {
            id,
            name: resource.name,
            mimeType: resource.mimeType,
            size: blob.bytes.length,
            md5Checksum: null,
            modifiedTime: '2026-08-24T00:00:00.000Z',
            parents: resource.parents,
            webViewLink: `https://drive.google.com/file/d/${id}/view`,
            driveId: null,
            appProperties: structuredClone(resource.appProperties),
          };
          files.set(id, file);
          return structuredClone(file);
        },
      },
    },
    Utilities: {
      DigestAlgorithm: { SHA_256: 'SHA_256' },
      Charset: { UTF_8: 'UTF_8' },
      base64Decode: (value) => [...Buffer.from(value, 'base64')],
      computeDigest(_algorithm, value) {
        const input = typeof value === 'string' ? Buffer.from(value, 'utf8') : Buffer.from(value);
        return [...createHash('sha256').update(input).digest()];
      },
      newBlob: (bytes, mimeType, name) => ({ bytes, mimeType, name }),
    },
  });

  for (const relative of ['src/00_Config.gs', 'src/01_Utils.gs', 'src/04_DriveBackup.gs']) {
    vm.runInContext(readFileSync(resolve(ROOT, relative), 'utf8'), context, { filename: relative });
  }
  context.ensureFolderStructure_ = () => ({ inbox: { id: 'inbox-folder' } });
  context.writeAuditLog_ = (eventType, payload) => {
    audits.push({ eventType, payload });
    return null;
  };

  function ingest(payload) {
    context.__payload = payload;
    return vm.runInContext('ingestHttpVideo_(__payload)', context);
  }

  return { audits, files, ingest };
}

function payloadFor(bytes, overrides = {}) {
  return {
    controlRequestId: 'youtube-e2e-ingest-fixture',
    fileName: 'anticaptrad-e2e-preview.mp4',
    mimeType: 'video/mp4',
    sha256: sha256(bytes),
    base64: bytes.toString('base64'),
    ...overrides,
  };
}

function assertCode(fn, expectedCode) {
  assert.throws(fn, (error) => {
    assert.equal(error.code, expectedCode);
    return true;
  });
}

test('authenticated HTTP ingest stores an integrity-pinned video once', () => {
  const harness = createHarness();
  const bytes = mp4Bytes('first');
  const result = harness.ingest(payloadFor(bytes));

  assert.equal(result.idempotentReplay, false);
  assert.equal(result.file.mimeType, 'video/mp4');
  assert.equal(result.file.size, bytes.length);
  assert.equal(result.contentSha256, sha256(bytes));
  assert.equal(harness.files.size, 1);
  assert.equal(harness.audits.length, 1);
  assert.equal(harness.audits[0].eventType, 'drive.http_video_ingested');

  const replay = harness.ingest(payloadFor(bytes, { fileName: 'renamed-on-retry.mp4' }));
  assert.equal(replay.idempotentReplay, true);
  assert.equal(replay.file.id, result.file.id);
  assert.equal(harness.files.size, 1);
  assert.equal(harness.audits.length, 1, 'an idempotent replay must not claim a second ingest');
});

test('reusing a control request for different bytes fails closed', () => {
  const harness = createHarness();
  harness.ingest(payloadFor(mp4Bytes('first')));
  assertCode(
    () => harness.ingest(payloadFor(mp4Bytes('second'), { fileName: 'different-name.mp4' })),
    'IDEMPOTENCY_CONFLICT',
  );
  assert.equal(harness.files.size, 1);
});

test('digest, MIME, signature, and decoded-size boundaries fail closed', () => {
  const bytes = mp4Bytes('valid');

  assertCode(
    () => createHarness().ingest(payloadFor(bytes, { sha256: '0'.repeat(64) })),
    'CONTENT_DIGEST_MISMATCH',
  );
  assertCode(
    () => createHarness().ingest(payloadFor(bytes, { mimeType: 'application/octet-stream' })),
    'INVALID_VIDEO_FILE',
  );

  const fake = Buffer.from('this is not a media container');
  assertCode(() => createHarness().ingest(payloadFor(fake)), 'INVALID_VIDEO_SIGNATURE');

  const oversized = Buffer.alloc(8 * 1024 * 1024 + 1);
  oversized.write('ftyp', 4, 'ascii');
  assertCode(() => createHarness().ingest(payloadFor(oversized)), 'VIDEO_SIZE_LIMIT');
});
