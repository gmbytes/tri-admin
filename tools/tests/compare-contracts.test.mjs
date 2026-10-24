import assert from 'node:assert/strict'
import test from 'node:test'
import { assertSafeFixture, compareContract } from '../compare-contracts.mjs'

test('contract comparison detects changed business errors and types', () => {
  assert.deepEqual(compareContract({code:0,data:{ID:7}}, {code:7,data:{ID:'7'}}).map(item => item.path), ['/code', '/data/ID'])
})

test('object key order is irrelevant but missing and additional fields are differences', () => {
  assert.deepEqual(compareContract({code:0,data:null}, {data:null,code:0}), [])
  assert.deepEqual(compareContract({ID:1}, {id:1}).map(item => item.path), ['/ID', '/id'])
})

test('array ordering, null, empty object and omitted field remain distinguishable', () => {
  assert.equal(compareContract([1,2], [2,1]).length, 2)
  assert.equal(compareContract(null, {}).length, 1)
  assert.equal(compareContract({data:null}, {}).length, 1)
})

test('only explicitly allowed volatile JSON pointers are normalized', () => {
  const expected = {data:{CreatedAt:'2026-01-01',ID:1}}
  const actual = {data:{CreatedAt:'2026-02-01',ID:2}}
  const differences = compareContract(expected, actual, ['/data/CreatedAt'])
  assert.deepEqual(differences.map(item => item.path), ['/data/ID'])
  assert.throws(() => compareContract(expected, actual, ['/data']), /volatile|normaliz/i)
})

test('JSON pointers escape slashes and tildes rather than ignoring a different field', () => {
  assert.deepEqual(compareContract({'a/b':{'~key':1}}, {'a/b':{'~key':2}}).map(item => item.path), ['/a~1b/~0key'])
})

test('fixture safety rejects nested credentials and bearer tokens', () => {
  for (const value of [{password:'not-for-storage'}, {data:{token:'secret'}}, {headers:{authorization:'Bearer test'}}, {url:'mysql://root:secret@localhost/app'}]) {
    assert.throws(() => assertSafeFixture(value), /credential|secret/i)
  }
  assert.doesNotThrow(() => assertSafeFixture({password:'<redacted>',data:{ID:3},headers:{'x-token':'<redacted>'}}))
})
