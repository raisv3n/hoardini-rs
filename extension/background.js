const HOST = "com.hoardini.host";

// ---------- hand-off to the native host ----------

// Like the browser, SameSite=Strict cookies only go along when the request starts on the cookie's own site
// (a page must not be able to make Hoardini fetch another site's URL with them). No referrer: not sent.
function fromCookieSite(cookie, referrer) {
  let host;
  try { host = new URL(referrer).hostname; } catch { return false; }
  const domain = cookie.domain.replace(/^\./, "");
  return host === domain || host.endsWith("." + domain);
}

async function cookieHeader(url, referrer) {
  const cookies = await chrome.cookies.getAll({ url });
  return cookies.filter(c => c.sameSite !== "strict" || fromCookieSite(c, referrer))
    .map(c => `${c.name}=${c.value}`).join("; ");
}

async function sendToHoardini({ url, referrer, kind = "file", filename = "", variant = "" }) {
  const res = await chrome.runtime.sendNativeMessage(HOST, {
    url, referrer: referrer || "", kind, filename, variant,
    cookies: await cookieHeader(url, referrer),
    userAgent: navigator.userAgent,
  });
  if (!res || !res.ok) throw new Error(res ? res.error : "no response from host");
}

async function enabled() {
  const { intercept = true } = await chrome.storage.local.get("intercept");
  return intercept;
}

// ---------- browser download capture ----------

chrome.downloads.onCreated.addListener(async item => {
  if (!(await enabled())) return;
  const url = item.finalUrl || item.url;
  if (!/^https?:/i.test(url)) return; // leave blob:/data:/file: alone
  try {
    await chrome.downloads.cancel(item.id);
    await chrome.downloads.erase({ id: item.id });
    await sendToHoardini({ url, referrer: item.referrer });
  } catch (e) {
    // Host missing or failed: restart the download in the browser so nothing is lost.
    console.warn("Hoardini handoff failed, falling back to browser:", e);
    chrome.downloads.download({ url });
  }
});

// ---------- media detection ----------

// Not offered: YouTube streams (site terms forbid downloading them outside YouTube's own features).
const BLOCKED_HOSTS = /(^|\.)(googlevideo\.com|youtube\.com|youtube-nocookie\.com|ytimg\.com)$/i;
const SEGMENT_EXT = /\.(ts|m4s|aac|cmfv|cmfa)$/i;
const FILE_EXT = /\.(mp4|m4v|webm|mov|mkv|mp3|m4a|ogg|wav)$/i;
const MIN_FILE_BYTES = 200 * 1024;

function classify(details, headers) {
  const url = new URL(details.url);
  if (BLOCKED_HOSTS.test(url.hostname)) return null;
  const ct = (headers["content-type"] || "").split(";")[0].trim().toLowerCase();
  const path = url.pathname;

  if (/\.m3u8$/i.test(path) || ct === "application/vnd.apple.mpegurl" || ct === "application/x-mpegurl") {
    return { kind: "hls", size: 0 };
  }
  if (SEGMENT_EXT.test(path) || ct === "video/mp2t") return null;

  const isMedia = ct.startsWith("video/") || ct.startsWith("audio/") || FILE_EXT.test(path);
  if (!isMedia) return null;

  // Range responses report the real total in Content-Range, not Content-Length.
  const range = (headers["content-range"] || "").match(/\/(\d+)$/);
  const size = range ? Number(range[1]) : Number(headers["content-length"] || 0);
  if (size && size < MIN_FILE_BYTES) return null;
  return { kind: "file", size };
}

// Per-tab list lives in session storage (the service worker can be torn down at any time).
let queue = Promise.resolve();
function updateTab(tabId, fn) {
  queue = queue.then(async () => {
    const key = `media:${tabId}`;
    const items = (await chrome.storage.session.get(key))[key] || [];
    const next = fn(items);
    await chrome.storage.session.set({ [key]: next });
    chrome.action.setBadgeText({ tabId, text: next.length ? String(next.length) : "" }).catch(() => {});
    chrome.action.setBadgeBackgroundColor({ tabId, color: "#2563eb" }).catch(() => {});
  }).catch(e => console.error(e));
  return queue;
}

chrome.webRequest.onHeadersReceived.addListener(async details => {
  if (details.tabId < 0 || details.statusCode >= 400) return;
  const headers = {};
  for (const h of details.responseHeaders || []) headers[h.name.toLowerCase()] = h.value || "";
  const found = classify(details, headers);
  if (!found) return;

  let tab;
  try { tab = await chrome.tabs.get(details.tabId); } catch { return; }
  const referrer = details.initiator ? details.initiator + "/" : tab.url;
  let isNew = false;
  await updateTab(details.tabId, items => {
    if (items.some(i => i.url === details.url)) return items;
    isNew = true;
    return [...items, { url: details.url, kind: found.kind, size: found.size, referrer, title: tab.title || "" }];
  });
  if (isNew && found.kind === "hls") enrichPlaylist(details.tabId, details.url);
}, { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "other"] }, ["responseHeaders"]);

// Reads an HLS playlist so the popup can group variants under their master and label qualities.
function parseMaster(text, base) {
  const lines = text.split(/\r?\n/).map(l => l.trim());
  if (!lines.some(l => l.startsWith("#EXT-X-STREAM-INF"))) return null;
  const variants = [];
  for (let i = 0; i < lines.length; i++) {
    if (!lines[i].startsWith("#EXT-X-STREAM-INF")) continue;
    const bw = Number((lines[i].match(/(?:^|[:,])BANDWIDTH=(\d+)/) || [])[1] || 0);
    const res = (lines[i].match(/RESOLUTION=(\d+)x(\d+)/) || []);
    const uri = lines.slice(i + 1).find(l => l && !l.startsWith("#"));
    if (!uri) continue;
    let url;
    try { url = new URL(uri, base).href; } catch { continue; }
    variants.push({ url, bandwidth: bw, height: Number(res[2] || 0),
      label: res[2] ? `${res[2]}p` : bw ? `${Math.round(bw / 1000)} kbps` : "stream" });
  }
  variants.sort((a, b) => b.bandwidth - a.bandwidth || b.height - a.height);
  return variants;
}

async function enrichPlaylist(tabId, url) {
  let variants = null;
  try {
    const res = await fetch(url, { credentials: "include" });
    if (res.ok) variants = parseMaster(await res.text(), url);
  } catch { /* leave unlabelled */ }
  await updateTab(tabId, items => items.map(i => i.url === url ? { ...i, variants } : i));
}

// A new page load starts a fresh list.
chrome.tabs.onUpdated.addListener((tabId, info) => {
  if (info.status === "loading" && info.url) updateTab(tabId, () => []);
});
chrome.tabs.onRemoved.addListener(tabId => chrome.storage.session.remove(`media:${tabId}`));

chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg.type === "list") {
    chrome.storage.session.get(`media:${msg.tabId}`).then(r => reply(r[`media:${msg.tabId}`] || []));
    return true;
  }
  if (msg.type === "download") {
    const { url, referrer, kind, title, variant } = msg.item;
    sendToHoardini({ url, referrer, kind, filename: title, variant })
      .then(() => reply({ ok: true }), e => reply({ ok: false, error: String(e.message || e) }));
    return true;
  }
});

// ---------- context menu ----------

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "hoardini-link", title: "Download with Hoardini", contexts: ["link", "video", "audio", "image"],
  });
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const url = info.linkUrl || info.srcUrl;
  if (!url || !/^https?:/i.test(url)) return;
  const kind = /\.m3u8(\?|$)/i.test(url) ? "hls" : "file";
  await sendToHoardini({ url, referrer: tab && tab.url, kind }).catch(e => console.error(e));
});
