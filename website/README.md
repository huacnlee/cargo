# Website

Static Astro site, built with Bun and deployed to GitHub Pages. English lives at
`/cargo/`, Simplified Chinese at `/cargo/zh-CN/`. All text is in
`src/lib/copy.ts`.

```sh
cd website
bun install --frozen-lockfile
bun run dev
bun run build
bun x playwright install chromium
bun run test
```

The default base is `/cargo`. Set `BASE_PATH` and `SITE_URL` together for another
host.

`public/install.sh` and `public/install.ps1` are symlinks to the repository root.
Their URLs are what users install from, so they must stay at `/cargo/install.sh`
and `/cargo/install.ps1`.

`.github/workflows/website.yml` builds and tests pull requests, then deploys
`main`.
