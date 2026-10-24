import { readFile,writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export function validateLegacySchema(schema) {
  if (!Array.isArray(schema.columns) || !schema.columns.length || !Array.isArray(schema.indexes)) throw new Error('Missing schema metadata')
  const columns = new Set()
  const tables = new Set()
  for (const column of schema.columns) {
    if (![column.table,column.name,column.type,column.nullable].every(value=>typeof value==='string'&&value.length)) throw new Error('Empty column metadata')
    const key = `${column.table}.${column.name}`
    if (columns.has(key)) throw new Error(`Duplicate column metadata: ${key}`)
    columns.add(key)
    tables.add(column.table)
  }
  for (const index of schema.indexes) {
    if (!index.name || !columns.has(`${index.table}.${index.column}`) || index.sequence < 1) throw new Error('Index references invalid column metadata')
  }
  return {tables:tables.size,columns:columns.size,indexFields:schema.indexes.length}
}

const relations = {
  sys_user_authority:{target:'sys_users',targetField:'authority_ids',ownerKey:'sys_user_id',valueKey:'sys_authority_authority_id'},
  sys_authority_menus:{target:'sys_authorities',targetField:'menu_ids',ownerKey:'sys_authority_authority_id',valueKey:'sys_base_menu_id'},
  sys_data_authority_id:{target:'sys_authorities',targetField:'data_authority_ids',ownerKey:'sys_authority_authority_id',valueKey:'data_authority_id_authority_id'},
  sys_authority_btns:{target:'sys_authorities',targetField:'button_permissions',ownerKey:'authority_id',valueKey:null},
}

export function buildMongoMapping(schema) {
  const counts = validateLegacySchema(schema)
  const tables = [...new Set(schema.columns.map(column=>column.table))].sort().map(source=>{
    const relation = relations[source]
    return {source,target:relation?.target ?? source,mode:relation?'embed-ids':'collection',...(relation ?? {}),
      fields:schema.columns.filter(column=>column.table===source).map(column=>({source:column.name,target:column.name,
        sourceType:column.type,nullable:column.nullable==='YES',conversion:column.type.startsWith('datetime')?'bson-date':column.type.includes('int')?'checked-int64':'preserve'}))}
  })
  return {version:1,source:'mysql',target:'mongodb',counts,tables,
    identifiers:{internal:'_id:ObjectId',external:'id:int64',sequences:'_counters; advance to maximum imported id',rejectOutOfRange:true},
    passwords:'preserve bcrypt hash without rehashing',softDelete:'deleted_at BSON Date or null',
    activation:'freeze source writes; validate counts, uniqueness and references; switch the whole domain'}
}

if (process.argv[1] && resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const schema=JSON.parse(await readFile('contracts/schema/legacy.json','utf8'))
  const mapping=buildMongoMapping(schema)
  await writeFile('contracts/mongo-mapping.json',`${JSON.stringify(mapping,null,2)}\n`)
  console.log(`Validated ${mapping.counts.tables} tables and mapped all ${mapping.counts.columns} source columns.`)
}
