// Runs in Facebook pages' own JavaScript world from document start. Facebook puts complete video files
// (and a DASH manifest) in its page data, both in inline <script> blocks and in the GraphQL responses that
// load a video when you click to it; it also removes some of those blocks once used. This collects the
// video fields as they go by into window.__hoardiniFB, which the popup reads.
(() => {
  if (window.__hoardiniFB) return;
  const store = window.__hoardiniFB = { videos: {}, titles: {} };

  const STR = '"((?:[^"\\\\]|\\\\.)*)"';
  // Older layout: browser_native_{sd,hd}_url + dash_manifest_xml_string. Newer one (videoDeliveryResponseFragment):
  // progressive_urls [{progressive_url, metadata: {quality: "HD"|"SD"}}] + dash_manifests [{manifest_xml}].
  const field = new RegExp('"(browser_native_sd_url|browser_native_hd_url|dash_manifest_xml_string|progressive_url|manifest_xml)":' + STR, "g");
  const KEYS = { browser_native_sd_url: "sd", browser_native_hd_url: "hd", dash_manifest_xml_string: "mpd", manifest_xml: "mpd" };
  const MARKERS = ["browser_native_", "dash_manifest_xml_string", "progressive_url", "manifest_xml"];
  const title = new RegExp('"title":\\{(?:[^{}"]|' + STR + ')*?"text":' + STR, "g");
  const idAt = /"id":"(\d+)"/g;
  const unescape = s => { try { return JSON.parse(`"${s}"`); } catch { return ""; } };

  // The video's "id" closes the object these fields sit in; fall back to the nearest one before them.
  function videoID(text, from, before) {
    idAt.lastIndex = from;
    const after = idAt.exec(text);
    if (after && after.index - from < 5000) return after[1];
    const ids = [...text.slice(Math.max(0, before - 2000), before).matchAll(/"id":"(\d+)"/g)];
    return ids.length ? ids[ids.length - 1][1] : "";
  }

  function scan(text) {
    if (typeof text !== "string") return;
    const hasVideo = MARKERS.some(k => text.includes(k));
    if (!hasVideo && !text.includes('"title":{')) return;
    if (hasVideo) {
      for (const m of text.matchAll(field)) {
        const end = m.index + m[0].length;
        let key = KEYS[m[1]];
        if (!key) {   // progressive_url: its quality follows in the same small object
          const q = text.slice(end, end + 300).match(/"quality":"(HD|SD)"/);
          key = q && q[1] === "HD" ? "hd" : "sd";
        }
        const id = videoID(text, end, m.index);
        const v = store.videos[id || "?"] || (store.videos[id || "?"] = {});
        if (!v[key]) v[key] = unescape(m[2]);
      }
    }
    // The title sits in another block, shortly after its video's "id".
    for (const m of text.matchAll(title)) {
      const ids = [...text.slice(Math.max(0, m.index - 300), m.index).matchAll(/"id":"(\d+)"/g)];
      if (ids.length && !store.titles[ids[ids.length - 1][1]]) store.titles[ids[ids.length - 1][1]] = unescape(m[2]);
    }
  }
  store.scan = scan;

  // Inline data blocks: the ones already there, and each one added later, read before Facebook removes it.
  const seen = new WeakSet();
  const scanScript = s => { if (!seen.has(s)) { seen.add(s); scan(s.textContent); } };
  document.querySelectorAll("script").forEach(scanScript);
  new MutationObserver(records => {
    for (const r of records) for (const n of r.addedNodes) {
      if (n.nodeName === "SCRIPT") scanScript(n);
      else if (n.querySelectorAll) n.querySelectorAll("script").forEach(scanScript);
    }
  }).observe(document, { childList: true, subtree: true });

  // GraphQL responses (clicking from the feed to a video loads it this way).
  const isGraphQL = u => /\/api\/graphql|\/ajax\/|graphql/.test(String(u));
  const open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    if (isGraphQL(url)) {
      this.addEventListener("load", () => {
        if (this.responseType === "" || this.responseType === "text") scan(this.responseText);
      });
    }
    return open.call(this, method, url, ...rest);
  };
  const fetch0 = window.fetch;
  window.fetch = function (input, init) {
    const p = fetch0.apply(window, arguments);
    const url = input && input.url || input;
    if (isGraphQL(url)) p.then(r => r.clone().text()).then(scan).catch(() => {});
    return p;
  };
})();
