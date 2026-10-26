/**
 * 按 artifact-metadata.json 校验产物目录。
 *
 *   bun checksum.ts <artifact-dir>
 *
 * 每个文件输出 ok 或 MISMATCH <path>；有不一致或缺失时 exit 1。
 */

const root = Bun.argv[2]
if (!root) {
  console.error('用法: bun checksum.ts <产物根目录>')
  process.exit(2)
}

const metaPath = `${root}/artifact-metadata.json`
const meta = (await Bun.file(metaPath).json()) as {
  version?: string
  releaseSha?: string
  files: { path: string; sha256: string; size: number }[]
}

let problemCount = 0
for (const file of meta.files) {
  const path = `${root}/${file.path}`
  const handle = Bun.file(path)
  if (!(await handle.exists())) {
    console.log(`MISSING ${file.path}`)
    problemCount++
    continue
  }
  const byteList = new Uint8Array(await handle.arrayBuffer())
  const digest = new Bun.CryptoHasher('sha256').update(byteList).digest('hex')
  if (digest !== file.sha256) {
    console.log(
      `MISMATCH ${file.path} (meta=${file.sha256.slice(0, 12)} disk=${digest.slice(0, 12)})`,
    )
    problemCount++
  } else if (byteList.byteLength !== file.size) {
    console.log(
      `SIZE ${file.path} (meta=${file.size} disk=${byteList.byteLength})`,
    )
    problemCount++
  } else {
    console.log(`ok ${file.path}`)
  }
}

console.log(
  JSON.stringify(
    {
      root,
      version: meta.version ?? null,
      releaseSha: meta.releaseSha ?? null,
      files: meta.files.length,
      problems: problemCount,
    },
    null,
    2,
  ),
)
process.exit(problemCount === 0 ? 0 : 1)
