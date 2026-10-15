const box = document.getElementById("intercept");
chrome.storage.local.get("intercept").then(({ intercept = true }) => { box.checked = intercept; });
box.addEventListener("change", () => chrome.storage.local.set({ intercept: box.checked }));

const list = document.getElementById("list");
const names = document.getElementById("names");

function fmtSize(n) {
  if (!n) return "";
  return n > 1e9 ? (n / 1e9).toFixed(2) + " GB" : (n / 1e6).toFixed(1) + " MB";
}

// Headings visible on the page are usually a better filename than the tab title.
async function nameCandidates(tab) {
  const out = [];
  try {
    const [{ result }] = await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => {
        const vis = el => el.getClientRects().length > 0;
        const text = [...document.querySelectorAll("h1, h2")].filter(vis)
          .map(e => e.textContent.trim().replace(/\s+/g, " ")).filter(t => t && t.length < 120);
        const og = document.querySelector('meta[property="og:title"]');
        return [...text, og ? og.content : "", document.title];
      },
    });
    out.push(...result);
  } catch { /* restricted page */ }
  out.push(tab.title || "");
  return [...new Set(out.map(s => s.trim()).filter(Boolean))];
}

// Facebook streams video in pieces, but its page data also holds complete MP4s (video + sound) in HD and SD.
// They are only in the HTML of a page that was loaded directly, not one reached by clicking around Facebook.
const FB_HOST = /(^|\.)facebook\.com$/i;

function facebookVideoID(pageURL) {
  const u = new URL(pageURL);
  return u.searchParams.get("v") || (u.pathname.match(/\/(?:videos|reel|watch)\/(?:[^/]+\/)?(\d{6,})/) || [])[1] || "";
}

async function facebookItems(tab) {
  if (!tab.url || !FB_HOST.test(new URL(tab.url).hostname)) return { items: [], isVideoPage: false };
  const id = facebookVideoID(tab.url);
  let found = [];
  try {
    // fb.js normally runs from page start (manifest); injecting it again is a no-op, and covers tabs
    // opened before the extension was loaded (those only see what is still in the page).
    await chrome.scripting.executeScript({ target: { tabId: tab.id }, world: "MAIN", files: ["fb.js"] });
    const [{ result }] = await chrome.scripting.executeScript({
      target: { tabId: tab.id }, world: "MAIN",
      func: () => {
        const s = window.__hoardiniFB;
        if (!s) return [];
        document.querySelectorAll("script").forEach(el => s.scan(el.textContent));
        return Object.entries(s.videos).map(([id, v]) => ({ id, ...v, title: s.titles[id] || "" }));
      },
    });
    // The store lives in the page's own world, so the page can rewrite it: keep plain strings only.
    const str = x => typeof x === "string" ? x : "";
    found = (Array.isArray(result) ? result : []).filter(f => f && typeof f === "object")
      .map(f => ({ id: str(f.id), hd: fbVideoURL(f.hd), sd: fbVideoURL(f.sd), mpd: str(f.mpd).slice(0, 1_000_000), title: str(f.title).slice(0, 300) }));
  } catch { /* restricted page */ }
  // A feed lists several videos; on a video's own page offer only that one. Fields whose video id could not
  // be worked out ("?") stand in only when nothing else is known about the page's video.
  if (id) {
    const own = found.filter(f => f.id === id);
    found = own.length ? own : found.filter(f => f.id === "?").map(f => ({ ...f, id }));
  }
  const seen = new Set();
  const items = found.filter(f => (f.hd || f.sd) && !seen.has(f.id) && seen.add(f.id)).map(f => {
    // Separate video + audio qualities (often up to 720p or 1080p) first, then the ready-made single files.
    const choices = [...dashChoices(f.mpd),
      f.hd && { label: "HD file", url: f.hd }, f.sd && { label: "SD file", url: f.sd }].filter(Boolean);
    return { url: choices[0].url, audio: choices[0].audio, kind: "file", referrer: tab.url,
      title: f.title || tab.title || "", size: 0, label: "Facebook video", choices };
  });
  return { items, isVideoPage: !!id };
}

// Video-only qualities from a DASH manifest, each paired with the best audio track. Hoardini merges the
// two into an MP4 without re-encoding, so only H.264/HEVC video with AAC audio is offered.
function dashChoices(xml) {
  if (!xml) return [];
  let doc;
  try { doc = new DOMParser().parseFromString(xml, "application/xml"); } catch { return []; }
  const reps = [...doc.getElementsByTagName("Representation")].map(r => ({
    mime: r.getAttribute("mimeType") || r.parentElement.getAttribute("mimeType") || "",
    codecs: r.getAttribute("codecs") || "",
    height: Number(r.getAttribute("height") || 0),
    bandwidth: Number(r.getAttribute("bandwidth") || 0),
    url: (r.getElementsByTagName("BaseURL")[0] || {}).textContent || "",
  })).map(r => ({ ...r, url: fbVideoURL(r.url) })).filter(r => r.url);
  const audio = reps.filter(r => r.mime.startsWith("audio/") && /^mp4a/.test(r.codecs))
    .sort((a, b) => b.bandwidth - a.bandwidth)[0];
  if (!audio) return [];
  const best = new Map();   // one choice per height: the highest bitrate
  for (const v of reps.filter(r => r.mime.startsWith("video/") && /^(avc1|hvc1|hev1)/.test(r.codecs) && r.height)) {
    if (!best.has(v.height) || best.get(v.height).bandwidth < v.bandwidth) best.set(v.height, v);
  }
  return [...best.values()].sort((a, b) => b.height - a.height)
    .map(v => ({ label: `${v.height}p`, url: v.url, audio: audio.url }));
}

// Facebook serves video files from its CDN only; anything else in the page data is not offered.
function fbVideoURL(u) {
  try {
    const x = new URL(u);
    return x.protocol === "https:" && /(^|\.)fbcdn\.net$/i.test(x.hostname) ? x.href : "";
  } catch { return ""; }
}

// Total size from a 1-byte range request; fine to skip when the server will not say.
async function fileSize(url) {
  try {
    const res = await fetch(url, { headers: { Range: "bytes=0-0" } });
    const m = (res.headers.get("content-range") || "").match(/\/(\d+)$/);
    res.body && res.body.cancel();
    return m ? Number(m[1]) : Number(res.headers.get("content-length") || 0);
  } catch { return 0; }
}

// Hide variant playlists that belong to a master we already know about.
function visibleItems(items) {
  const children = new Set(items.flatMap(i => (i.variants || []).map(v => v.url)));
  return items.filter(i => !children.has(i.url));
}

function row(item, candidates) {
  const el = document.createElement("div");
  el.className = "item";

  const nameInput = document.createElement("input");
  nameInput.type = "text";
  nameInput.setAttribute("list", "names");
  nameInput.value = (item.choices && item.title) || candidates[0] || new URL(item.url).pathname.split("/").pop();
  nameInput.title = "File name (click for other suggestions)";

  const actions = document.createElement("div");
  actions.className = "actions";

  let quality = null;
  if (item.variants && item.variants.length) {
    quality = document.createElement("select");
    quality.append(new Option("Best", ""));
    item.variants.forEach(v => quality.append(new Option(v.label, v.url)));
    quality.selectedIndex = 0;
  } else if (item.choices && item.choices.length > 1) {
    // Complete files in more than one quality: the choice is the URL itself.
    quality = document.createElement("select");
    item.choices.forEach(c => quality.append(new Option(c.label, c.url)));
  }

  const meta = document.createElement("div");
  meta.className = "meta";
  const showMeta = (url, size) => {
    meta.textContent = [item.label || (item.kind === "hls" ? "HLS stream" : "File"), fmtSize(size), new URL(url).hostname]
      .filter(Boolean).join(" · ");
    meta.title = url;
  };
  const selected = () => (item.choices && quality && item.choices[quality.selectedIndex]) || item;
  showMeta(item.url, item.size);
  if (item.choices) {
    const refresh = async () => {
      const c = selected();
      const sizes = await Promise.all([fileSize(c.url), c.audio ? fileSize(c.audio) : 0]);
      showMeta(c.url, sizes[0] && sizes[0] + sizes[1]);
    };
    quality && quality.addEventListener("change", refresh);
    refresh();
  }

  const btn = document.createElement("button");
  btn.textContent = "Download";
  btn.addEventListener("click", async () => {
    btn.disabled = true;
    // Send the master, not the variant: only the master lists a separate audio track (e.g. Vimeo).
    const chosen = item.choices
      ? { ...item, url: selected().url, audio: selected().audio || "", variant: "", title: nameInput.value.trim() }
      : { ...item, variant: (quality && quality.value) || "", title: nameInput.value.trim() };
    const res = await chrome.runtime.sendMessage({ type: "download", item: chosen });
    btn.textContent = res && res.ok ? "Sent ✓" : "Failed";
    if (res && !res.ok) { btn.title = res.error; btn.disabled = false; }
  });

  actions.append(...[meta, quality, btn].filter(Boolean));
  el.append(nameInput, actions);
  return el;
}

(async () => {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const detected = tab ? visibleItems(await chrome.runtime.sendMessage({ type: "list", tabId: tab.id })) : [];
  const fb = tab ? await facebookItems(tab) : { items: [], isVideoPage: false };
  const items = [...fb.items, ...detected];
  list.textContent = "";
  if (!items.length) {
    list.className = "empty";
    list.textContent = fb.isVideoPage
      ? "Hoardini has not seen this Facebook video's details yet. Reload the page, then reopen this popup."
      : "No media detected yet. Press play, then reopen this popup.";
    return;
  }
  list.className = "";
  const candidates = await nameCandidates(tab);
  for (const c of candidates) names.append(new Option(c));
  for (const item of items) list.append(row(item, candidates));
})();
