// Builds the pool of shortcodes that load.js reads from.
//
//   node infra/k6/seed.mjs --base http://host:30091 --count 100000 > pool.json
//
// Not a k6 script, deliberately. Each k6 VU runs in its own JavaScript runtime,
// so values collected inside a VU never reach handleSummary — a seeder has to
// return data, and k6 is built to return measurements. Node also gets this done
// with plain concurrency and no extra dependency.
//
// Seeding is itself a write load. Keeping it out of the measured run is what
// stops a cold cache and a warm one from landing in the same numbers.

const args = new Map()
for (let i = 2; i < process.argv.length; i += 2) {
  args.set(process.argv[i].replace(/^--/, ''), process.argv[i + 1])
}

const BASE = args.get('base') ?? 'http://127.0.0.1:3000'
const COUNT = Number(args.get('count') ?? 100_000)
const CONCURRENCY = Number(args.get('concurrency') ?? 64)

const codes = []
let attempted = 0
let failed = 0

// The API is idempotent by long URL: the same URL always returns the same code.
// Repeating one would silently shrink the pool, so every URL carries its index.
// The padding varies the length a little, since a fixed-size body would make
// every row identical in width and hide any size effect.
function longUrl(n) {
  return `https://example.com/seed/${n}/${'x'.repeat(n % 40)}`
}

async function worker() {
  for (;;) {
    const n = attempted++
    if (n >= COUNT) return

    try {
      const res = await fetch(`${BASE}/v1/shorten`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ url: longUrl(n) }),
      })

      if (res.status === 429) {
        // The rate limit is per IP and a seeder is one IP pretending to be many.
        // Failing loudly here beats producing a short pool that looks complete.
        throw new Error('rate limited — is this IP exempt from the write limit?')
      }
      if (!res.ok) {
        failed++
        continue
      }

      const body = await res.json()
      if (body.code) codes.push(body.code)
    } catch (err) {
      if (String(err.message).includes('rate limited')) {
        process.stderr.write(`\n${err.message}\n`)
        process.exit(1)
      }
      failed++
    }

    if (codes.length % 5000 === 0 && codes.length > 0) {
      process.stderr.write(`\r${codes.length}/${COUNT} created`)
    }
  }
}

const started = Date.now()
await Promise.all(Array.from({ length: CONCURRENCY }, worker))
const elapsed = (Date.now() - started) / 1000

process.stderr.write(
  `\r${codes.length} created, ${failed} failed, ${elapsed.toFixed(1)}s ` +
    `(${Math.round(codes.length / elapsed)}/s)\n`,
)

// Sorted so the Zipf draw in load.js is reproducible: the hot end of the
// distribution has to be the same set on every run, or two runs are not
// comparable.
codes.sort()

process.stdout.write(JSON.stringify({ base: BASE, count: codes.length, codes }))
