// One stored ZIP entry for independent package-format fixtures; no host extraction.
export function pluginArchive(manifest: Record<string, unknown>): Buffer {
  const root = manifest.name
  if (typeof root !== 'string' || root.length > 64 || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(root))
    throw Error('Invalid fixture root')
  const name = Buffer.from(`${root}/tri-plugin.json`),
    data = Buffer.from(JSON.stringify(manifest))
  if (data.length > 256 * 1024) throw Error('Fixture manifest too large')
  let crc = 0xffffffff
  for (const byte of data) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0)
  }
  crc = (crc ^ 0xffffffff) >>> 0
  const local = Buffer.alloc(30 + name.length),
    central = Buffer.alloc(46 + name.length),
    end = Buffer.alloc(22)
  local.writeUInt32LE(0x04034b50, 0)
  local.writeUInt16LE(20, 4)
  local.writeUInt16LE(0x800, 6)
  local.writeUInt32LE(crc, 14)
  local.writeUInt32LE(data.length, 18)
  local.writeUInt32LE(data.length, 22)
  local.writeUInt16LE(name.length, 26)
  name.copy(local, 30)
  central.writeUInt32LE(0x02014b50, 0)
  central.writeUInt16LE((3 << 8) | 20, 4)
  central.writeUInt16LE(20, 6)
  central.writeUInt16LE(0x800, 8)
  central.writeUInt32LE(crc, 16)
  central.writeUInt32LE(data.length, 20)
  central.writeUInt32LE(data.length, 24)
  central.writeUInt16LE(name.length, 28)
  central.writeUInt32LE((0o100644 * 65536) >>> 0, 38)
  name.copy(central, 46)
  end.writeUInt32LE(0x06054b50, 0)
  end.writeUInt16LE(1, 8)
  end.writeUInt16LE(1, 10)
  end.writeUInt32LE(central.length, 12)
  end.writeUInt32LE(local.length + data.length, 16)
  return Buffer.concat([local, data, central, end])
}
