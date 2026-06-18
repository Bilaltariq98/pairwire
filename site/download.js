/**
 * download.js — Pairwire landing site download button
 *
 * Static fallback (no JS required):
 *   The <a> button already carries a working href pointing at the
 *   stable asset URL pattern. This script ENHANCES that:
 *   1. Fetches the latest GitHub Release via the API
 *   2. Finds the first .dmg asset and updates the button href to its
 *      browser_download_url
 *   3. Shows the version (from tag_name) in the version label
 *   On any error the button keeps its static href and the version label
 *   stays hidden — fail silent.
 *
 * NOTE FOR RELEASE CI:
 *   release.yml must upload the DMG asset as a stable name
 *   "pairwire-macos.dmg" so the no-JS static path always resolves.
 *   The API path finds any .dmg regardless of exact filename.
 */

/* Static fallback — must match the href in index.html */
const STATIC_DMG_URL =
  'https://github.com/Bilaltariq98/pairwire/releases/latest/download/pairwire-macos.dmg';

const RELEASES_API =
  'https://api.github.com/repos/Bilaltariq98/pairwire/releases/latest';

(function initDownload() {
  document.addEventListener('DOMContentLoaded', function () {
    const btn = document.getElementById('download-btn');
    const versionEl = document.getElementById('version-label');

    if (!btn) return;

    /* btn already has the static href — fetch to enhance */
    fetchLatestRelease(btn, versionEl);
  });
}());

function fetchLatestRelease(btn, versionEl) {
  fetch(RELEASES_API, {
    headers: { Accept: 'application/vnd.github+json' },
    /* 8-second timeout — keep it snappy, don't block the user */
    signal: AbortSignal.timeout ? AbortSignal.timeout(8000) : undefined,
  })
    .then(function (res) {
      if (!res.ok) throw new Error('Non-OK: ' + res.status);
      return res.json();
    })
    .then(function (release) {
      var assets = release.assets || [];
      var dmgAsset = assets.find(function (a) {
        return a.name && a.name.toLowerCase().endsWith('.dmg');
      });

      if (dmgAsset && dmgAsset.browser_download_url) {
        btn.href = dmgAsset.browser_download_url;
      }
      /* Even if no asset found yet, show the version tag */
      var tag = release.tag_name || '';
      var version = tag.replace(/^v/, '');
      if (version && versionEl) {
        versionEl.textContent = tag; /* keep the v-prefix for display */
        versionEl.removeAttribute('hidden');
        /* reveal the leading separator only once there's a version after it */
        var sep = document.querySelector('.version-sep');
        if (sep) sep.removeAttribute('hidden');
      }
    })
    .catch(function () {
      /* Fail silent: btn keeps static href, versionEl stays hidden */
    });
}
