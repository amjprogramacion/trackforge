#!/usr/bin/env node

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const scriptsDirectory = dirname(fileURLToPath(import.meta.url))
const root = resolve(scriptsDirectory, '..')
const packageJson = JSON.parse(
  readFileSync(resolve(root, 'package.json'), 'utf8'),
)
const version = packageJson.version
const tag = process.env.RELEASE_TAG

if (!tag) {
  throw new Error('RELEASE_TAG is required.')
}

if (!/^\d+\.\d+\.\d+$/.test(version)) {
  throw new Error(
    `Invalid package version "${version}"; expected MAJOR.MINOR.PATCH.`,
  )
}

if (tag !== `v${version}`) {
  throw new Error(
    `Release tag "${tag}" does not match package version "${version}".`,
  )
}

console.log(`[release-check] ${tag} matches package version ${version}`)
