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
  nameInput.value = candidates[0] || new URL(item.url).pathname.split("/").pop();
  nameInput.title = "File name (click for other suggestions)";

  const actions = document.createElement("div");
  actions.className = "actions";

  let quality = null;
  if (item.variants && item.variants.length) {
    quality = document.createElement("select");
    quality.append(new Option("Best", ""));
    item.variants.forEach(v => quality.append(new Option(v.label, v.url)));
    quality.selectedIndex = 0;
  }

  const meta = document.createElement("div");
  meta.className = "meta";
  meta.textContent = [item.kind === "hls" ? "HLS stream" : "File", fmtSize(item.size), new URL(item.url).hostname]
    .filter(Boolean).join(" · ");
  meta.title = item.url;

  const btn = document.createElement("button");
  btn.textContent = "Download";
  btn.addEventListener("click", async () => {
    btn.disabled = true;
    // Send the master, not the variant: only the master lists a separate audio track (e.g. Vimeo).
    const chosen = { ...item, variant: (quality && quality.value) || "", title: nameInput.value.trim() };
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
  const items = tab ? visibleItems(await chrome.runtime.sendMessage({ type: "list", tabId: tab.id })) : [];
  list.textContent = "";
  if (!items.length) {
    list.className = "empty";
    list.textContent = "No media detected yet. Press play, then reopen this popup.";
    return;
  }
  list.className = "";
  const candidates = await nameCandidates(tab);
  for (const c of candidates) names.append(new Option(c));
  for (const item of items) list.append(row(item, candidates));
})();
