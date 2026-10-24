import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { validateLegacySchema,buildMongoMapping } from '../schema-mapping.mjs'

const column = {table:'sys_users',name:'id',type:'bigint unsigned',nullable:'NO',default:null,extra:'auto_increment'}

test('rejects empty metadata aliases instead of treating row counts as a valid schema', () => {
  assert.throws(()=>validateLegacySchema({columns:[{...column,table:'',name:''}],indexes:[]}),/metadata|column/i)
})

test('rejects indexes referencing nonexistent columns', () => {
  assert.throws(()=>validateLegacySchema({columns:[column],indexes:[{table:'sys_users',name:'wrong',column:'missing',sequence:1,non_unique:0}]}),/index/i)
})

test('captured source schema contains actual user fields and every index references a column', async () => {
  const schema = JSON.parse(await readFile(new URL('../../contracts/schema/legacy.json',import.meta.url),'utf8'))
  const report = validateLegacySchema(schema)
  assert.ok(report.tables >= 30)
  assert.ok(schema.columns.some(item=>item.table==='sys_users' && item.name==='username' && item.type.startsWith('varchar')))
})

test('MongoDB mapping accounts for every source column and embeds user role relationships', () => {
  const schema = {columns:[column,{...column,table:'sys_user_authority',name:'sys_user_id'},{...column,table:'sys_user_authority',name:'sys_authority_authority_id'}],indexes:[]}
  const mapping = buildMongoMapping(schema)
  assert.equal(mapping.tables.length,2)
  const relation = mapping.tables.find(item=>item.source==='sys_user_authority')
  assert.equal(relation.target,'sys_users')
  assert.equal(relation.mode,'embed-ids')
  assert.equal(relation.targetField,'authority_ids')
  assert.deepEqual(relation.fields.map(item=>item.source).sort(),['sys_authority_authority_id','sys_user_id'])
})
