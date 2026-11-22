const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
class SourceBuffer {
  constructor() { this.buffered = {length: 0}; this.updating = false; }
  appendBuffer() {}
}
class MediaSource {
  addSourceBuffer() { return new SourceBuffer(); }
  endOfStream() {}
}
class HTMLMediaElement {}
const store = {getState: () => ({player: {currentTrack: {id: 42}}}), dispatch() {}};
const root = {__reactFiberMock: {memoizedProps: {store}}};
let queueVersion = 0;
const context = {MediaSource, SourceBuffer, HTMLMediaElement, ArrayBuffer, Uint8Array, DataView,
  btoa: value => Buffer.from(value, 'latin1').toString('base64'),
  document: {getElementById: () => root},
  window: {__zbqMods: [{getCurrentPlayqueueVersion: () => queueVersion}],
    __zbq: {setMaxFormat() {}, play() {}, pause() {}}}};
vm.runInNewContext(fs.readFileSync(__dirname + '/../capture.js', 'utf8'), context);
context.window.__relay.begin(42);
const previous = new MediaSource().addSourceBuffer('audio/flac');
previous.appendBuffer(new Uint8Array([99]));
assert.equal(context.window.__relay.drain().data, '');
const buffer = new MediaSource().addSourceBuffer('audio/flac');
queueVersion++;
previous.appendBuffer(new Uint8Array([98]));
const expected = [];
for (let i = 0; i < 80; i++) {
  const bytes = new Uint8Array(20000).fill(i);
  expected.push(Buffer.from(bytes)); buffer.appendBuffer(bytes); bytes.fill(255);
}
const first = context.window.__relay.drain();
const second = context.window.__relay.drain();
assert.equal(Buffer.from(first.data, 'base64').length, 1024 * 1024);
assert.equal(second.queued, 0);
assert.deepEqual(Buffer.concat([Buffer.from(first.data, 'base64'), Buffer.from(second.data, 'base64')]), Buffer.concat(expected));
const oldSource = new MediaSource();
oldSource.addSourceBuffer('audio/flac');
const activeSource = new MediaSource();
activeSource.addSourceBuffer('audio/flac');
oldSource.endOfStream();
assert.equal(context.window.__relay.drain().ended, false);
activeSource.endOfStream();
assert.equal(context.window.__relay.drain().ended, true);
console.log('Capture batching preserves small append buffers byte for byte');

const stored = new Map();
context.localStorage = {setItem: (key, value) => stored.set(key, value), getItem: key => stored.get(key)};
context.window.__zbqMods = [{STORAGE_USER_KEY: 'localuser'}];
const account = {infos: {id: 42, name: 'Fixture'}, token: 'fixture-session', loading: true};
store.getState = () => ({user: account});
assert.equal(context.window.__relay.rememberSession(), true);
assert.deepEqual(JSON.parse(stored.get('localuser')), {id: 42, token: 'fixture-session', name: 'Fixture'});
store.getState = () => ({user: {id: 42}});
assert.equal(context.window.__relay.rememberSession(), false);
console.log('Browser session retention uses official storage and omits transient state');

(async () => {
  const events = [];
  let completeMetadata;
  const metadata = new Promise(resolve => { completeMetadata = resolve; });
  const relay = context.window.__relay;
  relay.metadata = () => { events.push('metadata requested'); return metadata; };
  relay.begin = (id, quality) => { assert.equal(id, '42'); assert.equal(quality, 27); events.push('media requested'); };
  context.window.__zbq.seek = position => { assert.equal(position, 0); events.push('seek to start'); };
  let completed = false;
  const preparing = relay.prepare('42', 27).then(value => { completed = true; return value; });
  await Promise.resolve();
  assert.deepEqual(events, ['metadata requested', 'media requested', 'seek to start']);
  assert.equal(completed, false);
  const expected = {available: true, duration_ms: 180000};
  completeMetadata(expected);
  assert.equal(await preparing, expected);
  console.log('Media starts while metadata is pending; preparation still awaits metadata');
})().catch(error => { console.error(error); process.exitCode = 1; });
