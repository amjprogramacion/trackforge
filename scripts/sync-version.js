#!/usr/bin/env node

import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const scriptsDirectory = dirname(fileURLToPath(import.meta.url))
const root = resolve(scriptsDirectory, '..')
const packageJsonPath = resolve(root, 'package.json')
const packageJson = JSON.parse(readFileSync(packageJsonPath, 'utf8'))
const version = packageJson.version

if (!/^\d+\.\d+\.\d+$/.test(version)) {
  throw new Error(
    `Invalid version "${version}" in package.json; expected MAJOR.MINOR.PATCH.`,
  )
}

console.log(`[sync-version] Syncing version ${version}`)

function updateJson(path, update) {
  const value = JSON.parse(readFileSync(path, 'utf8'))
  update(value)
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`)
}

function updateCargoManifest(relativePath) {
  const path = resolve(root, relativePath)
  const manifest = readFileSync(path, 'utf8').replace(
    /^version = "[^"]+"/m,
    `version = "${version}"`,
  )
  writeFileSync(path, manifest)
  console.log(`[sync-version] Updated ${relativePath}`)
}

updateJson(resolve(root, 'src-tauri/tauri.conf.json'), (config) => {
  config.version = version
})
console.log('[sync-version] Updated src-tauri/tauri.conf.json')

updateCargoManifest('Cargo.toml')
updateCargoManifest('src-tauri/Cargo.toml')

execFileSync(
  'cargo',
  ['update', '--precise', version, '--package', 'trackforge'],
  { cwd: root, stdio: 'inherit' },
)
console.log('[sync-version] Updated Cargo.lock')

execFileSync(
  'cargo',
  ['update', '--precise', version, '--package', 'trackforge-tauri'],
  { cwd: resolve(root, 'src-tauri'), stdio: 'inherit' },
)
console.log('[sync-version] Updated src-tauri/Cargo.lock')

updateJson(resolve(root, 'package-lock.json'), (lockfile) => {
  lockfile.version = version
  if (lockfile.packages?.['']) {
    lockfile.packages[''].version = version
  }
})
console.log('[sync-version] Updated package-lock.json')
console.log(`[sync-version] Done; all files use ${version}`)
