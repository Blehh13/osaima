import { test } from 'node:test';
import assert from 'node:assert/strict';
import { RagEngine, SEED_KNOWLEDGE } from '../src/wm/rag.js';

function engineWith(docs) {
  const rag = new RagEngine();
  for (const d of docs) rag.ingest(d);
  rag.buildIndex();
  return rag;
}

test('retrieves the most relevant document first', () => {
  const rag = engineWith([
    { title: 'Networking', text: 'Use NetworkManager and nmcli to connect to wifi networks.' },
    { title: 'Packages', text: 'Install software with emerge from the Portage tree.' },
    { title: 'Display', text: 'Adjust screen brightness from the control center.' },
  ]);
  const hits = rag.query('how do I install software packages', 3);
  assert.ok(hits.length > 0);
  assert.equal(hits[0].title, 'Packages');
  for (let i = 1; i < hits.length; i++) assert.ok(hits[i - 1].score >= hits[i].score);
});

test('unrelated or empty queries return nothing', () => {
  const rag = engineWith([{ title: 'A', text: 'window manager layouts tile monocle' }]);
  assert.deepEqual(rag.query(''), []);
  assert.deepEqual(rag.query('zebra quantum pancake'), []);
  assert.match(rag.answer('zebra quantum pancake').answer, /don't have anything/);
});

test('seed knowledge indexes and answers with sources', () => {
  const rag = engineWith(SEED_KNOWLEDGE);
  const { docs, chunks } = rag.stats();
  assert.equal(docs, SEED_KNOWLEDGE.length);
  assert.ok(chunks >= docs);
  const res = rag.answer('window manager layout');
  assert.ok(res.sources.length > 0);
  assert.equal(typeof res.answer, 'string');
});
