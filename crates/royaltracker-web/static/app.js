// Cruise Tracker Mini App — Preact + HTM, no build step.
// All views in one file for now; split into modules if it grows past ~800 lines.

import { h, render } from "https://esm.sh/preact@10.25.4";
import { useState, useEffect, useMemo, useRef, useCallback }
  from "https://esm.sh/preact@10.25.4/hooks";
import htm from "https://esm.sh/htm@3.1.1";

const html = htm.bind(h);

// ─── constants ───────────────────────────────────────────────────────────────

const tg = window.Telegram?.WebApp;
if (tg) { tg.ready(); tg.expand(); }
const initData = tg?.initData || "";

const SHIP_NAMES = {
  // Celebrity
  AS: "Celebrity Ascent", AX: "Celebrity Apex", BE: "Celebrity Beyond",
  EC: "Celebrity Eclipse", ED: "Celebrity Edge", EQ: "Celebrity Equinox",
  IN: "Celebrity Infinity", MI: "Celebrity Millennium", RF: "Celebrity Reflection",
  SI: "Celebrity Silhouette", SL: "Celebrity Solstice", SU: "Celebrity Summit",
  XC: "Celebrity Xcel",
  // Royal Caribbean
  AD: "Adventure of the Seas", AL: "Allure of the Seas", AN: "Anthem of the Seas",
  BR: "Brilliance of the Seas", EN: "Enchantment of the Seas", EP: "Explorer of the Seas",
  FR: "Freedom of the Seas", GR: "Grandeur of the Seas", HM: "Harmony of the Seas",
  IC: "Icon of the Seas", JW: "Jewel of the Seas",
  LB: "Liberty of the Seas", MA: "Mariner of the Seas", NV: "Navigator of the Seas",
  OA: "Oasis of the Seas", OD: "Odyssey of the Seas", OV: "Ovation of the Seas",
  QN: "Quantum of the Seas", RD: "Radiance of the Seas", RH: "Rhapsody of the Seas",
  SP: "Spectrum of the Seas", SR: "Star of the Seas", SY: "Symphony of the Seas",
  UT: "Utopia of the Seas", VS: "Vision of the Seas", VY: "Voyager of the Seas",
  WN: "Wonder of the Seas",
};
const BRAND_LABEL = { royal: "Royal Caribbean", celebrity: "Celebrity" };

const shipName = (c) => SHIP_NAMES[c] || c;
const brandLabel = (b) => BRAND_LABEL[b] || b;
const fmtSailDate = (iso) =>
  new Date(iso + "T12:00:00Z").toLocaleDateString(undefined,
    { year: "numeric", month: "long", day: "numeric" });
const daysUntil = (iso) => {
  const days = Math.round((new Date(iso + "T12:00:00Z").getTime() - Date.now()) / 86400000);
  if (days < 0) return `${-days} days ago`;
  if (days === 0) return "today!";
  if (days === 1) return "tomorrow";
  return `${days} days away`;
};
const priceText = (r) =>
  r.price_label
    ? `${r.price_label}${r.unit_label ? ` · ${r.unit_label}` : ""}`
    : (r.starting_price ? `${r.starting_price} ${r.currency || ""}` : "no price yet");

const tgColor = (varName, fallback) => {
  const c = getComputedStyle(document.body).getPropertyValue(varName).trim();
  return c || fallback;
};
const fmtDateTime = (iso) =>
  new Date(iso).toLocaleString(undefined, {
    month: "short", day: "numeric", year: "numeric",
    hour: "numeric", minute: "2-digit",
  });
const fmtDateShort = (iso) =>
  new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric" });

// ─── API ─────────────────────────────────────────────────────────────────────

async function api(path, opts = {}) {
  const headers = { "X-Telegram-Init-Data": initData, ...(opts.headers || {}) };
  if (opts.body && typeof opts.body !== "string") {
    headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(opts.body);
  }
  const r = await fetch(path, { ...opts, headers });
  if (!r.ok) {
    const text = await r.text().catch(() => r.statusText);
    throw new Error(`${r.status}: ${text}`);
  }
  return r.status === 204 ? null : await r.json();
}

// ─── shared hooks ────────────────────────────────────────────────────────────

function useBackButton(handler, deps) {
  useEffect(() => {
    if (!handler || !tg?.BackButton) return;
    tg.BackButton.show();
    tg.BackButton.onClick(handler);
    return () => {
      tg.BackButton.offClick(handler);
      tg.BackButton.hide();
    };
  // eslint-disable-next-line
  }, deps);
}

// ─── components ──────────────────────────────────────────────────────────────

function Header({ user }) {
  const sub = user
    ? `${user.tg_first_name || user.tg_username || "you"} · ${user.rcg_username}`
    : "Welcome — link your RCG account to begin.";
  return html`
    <header class="flex items-center justify-between">
      <div>
        <h1 class="text-xl font-semibold">🛳️ Cruise Tracker</h1>
        <p class="text-sm text-tg-hint">${sub}</p>
      </div>
    </header>`;
}

function RegisterForm({ onRegistered }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [status, setStatus] = useState({ tone: "hint", text: "" });
  const [busy, setBusy] = useState(false);

  async function submit(e) {
    e.preventDefault();
    setBusy(true);
    setStatus({ tone: "hint", text: "Linking…" });
    try {
      const r = await api("/api/register", { method: "POST", body: { email, password } });
      if (r.login_ok) {
        tg?.HapticFeedback?.notificationOccurred?.("success");
        setStatus({ tone: "hint", text: `✅ Linked. ${r.bookings_discovered} booking(s) found.` });
        setPassword("");
        await onRegistered();
      } else {
        tg?.HapticFeedback?.notificationOccurred?.("warning");
        setStatus({ tone: "warn", text: r.message });
      }
    } catch (err) {
      tg?.HapticFeedback?.notificationOccurred?.("error");
      setStatus({ tone: "err", text: err.message });
    } finally {
      setBusy(false);
    }
  }

  const toneClass = {
    hint: "text-tg-hint", warn: "text-yellow-500", err: "text-red-500",
  }[status.tone];

  return html`
    <section class="space-y-3">
      <div class="bg-tg-secondary rounded-lg p-4 space-y-3">
        <h2 class="text-lg font-medium">Link your account</h2>
        <p class="text-sm text-tg-hint">
          Enter your Royal Caribbean or Celebrity Cruises login. Your password is
          encrypted at rest and only used to fetch your personalized add-on prices
          on your behalf, once per day.
        </p>
        <form onSubmit=${submit} class="space-y-3" autocomplete="on">
          <label class="block">
            <span class="text-sm">Email</span>
            <input type="email" required autocomplete="username" value=${email}
                   onInput=${(e) => setEmail(e.target.value)}
                   class="mt-1 w-full bg-tg-bg text-tg-text rounded-lg px-3 py-2 border border-tg-hint/30
                          focus:outline-none focus:ring-2 focus:ring-tg-button" />
          </label>
          <label class="block">
            <span class="text-sm">Password</span>
            <input type="password" required autocomplete="current-password" value=${password}
                   onInput=${(e) => setPassword(e.target.value)}
                   class="mt-1 w-full bg-tg-bg text-tg-text rounded-lg px-3 py-2 border border-tg-hint/30
                          focus:outline-none focus:ring-2 focus:ring-tg-button" />
          </label>
          <p class="text-xs text-tg-hint">
            One Royal Caribbean account works for both Royal and Celebrity — I'll find
            bookings across both brands automatically.
          </p>
          <button type="submit" disabled=${busy}
                  class="w-full bg-tg-button text-tg-btext rounded-lg px-3 py-2.5 font-medium hover:opacity-90 transition disabled:opacity-50">
            ${busy ? "Linking…" : "Link account"}
          </button>
          <p class="text-sm ${toneClass}">${status.text}</p>
        </form>
      </div>
    </section>`;
}

function BookingsList({ bookings, onOpen, onRefresh }) {
  const [refreshing, setRefreshing] = useState(false);
  const [refreshLabel, setRefreshLabel] = useState("↻ Refresh");

  async function refresh() {
    setRefreshing(true);
    setRefreshLabel("↻ refreshing…");
    try {
      const r = await api("/api/refresh", { method: "POST" });
      tg?.HapticFeedback?.notificationOccurred?.("success");
      setRefreshLabel(`↻ ${r.bookings_discovered} booking${r.bookings_discovered === 1 ? "" : "s"}`);
      await onRefresh();
    } catch (e) {
      tg?.HapticFeedback?.notificationOccurred?.("error");
      setRefreshLabel("↻ failed");
    } finally {
      setRefreshing(false);
      setTimeout(() => setRefreshLabel("↻ Refresh"), 2000);
    }
  }

  return html`
    <section class="space-y-3">
      <div class="flex items-center justify-between">
        <h2 class="text-sm uppercase tracking-wide text-tg-hint">Your bookings</h2>
        <button onClick=${refresh} disabled=${refreshing} class="text-tg-link text-sm">${refreshLabel}</button>
      </div>
      <div class="space-y-2">
        ${bookings.length === 0
          ? html`<div class="bg-tg-secondary rounded-lg p-4 text-sm text-tg-hint">
              No bookings yet — tap <em>↻ Refresh</em> to pull them from your account.
            </div>`
          : bookings.map((b) => html`
              <button key=${b.reservation_id} onClick=${() => onOpen(b)}
                      class="w-full text-left bg-tg-secondary rounded-lg p-4 hover:bg-opacity-70 transition">
                <div class="flex justify-between items-start gap-2">
                  <div>
                    <div class="font-semibold">${shipName(b.ship_code)}</div>
                    <div class="text-sm text-tg-hint">
                      ${[b.nights && `${b.nights} night${b.nights === 1 ? "" : "s"}`, fmtSailDate(b.sail_date)].filter(Boolean).join(" · ")}
                    </div>
                  </div>
                  <div class="text-right">
                    <div class="text-xs text-tg-hint">${brandLabel(b.brand)}</div>
                    <div class="text-xs text-tg-hint mt-0.5">${daysUntil(b.sail_date)}</div>
                  </div>
                </div>
              </button>`)
        }
      </div>
    </section>`;
}

function BookingHeader({ booking }) {
  const nights = booking.nights ? `${booking.nights} night${booking.nights === 1 ? "" : "s"}` : "";
  return html`
    <div class="bg-tg-secondary rounded-lg p-4 space-y-1">
      <div class="flex justify-between items-start">
        <div>
          <div class="text-lg font-semibold">${shipName(booking.ship_code)}</div>
          <div class="text-sm text-tg-hint">${[nights, fmtSailDate(booking.sail_date)].filter(Boolean).join(" · ")}</div>
          <div class="text-xs text-tg-hint mt-1">${daysUntil(booking.sail_date)}</div>
        </div>
        <div class="text-xs text-tg-hint">${brandLabel(booking.brand)}</div>
      </div>
    </div>`;
}

function WatchedList({ watched, booking, onOpen }) {
  const mine = watched.filter((w) => w.reservation_id === booking.reservation_id);
  if (mine.length === 0) return null;
  return html`
    <div class="space-y-2">
      <h3 class="text-sm uppercase tracking-wide text-tg-hint">Tracking</h3>
      ${mine.map((w) => html`
        <button key=${w.id} onClick=${() => onOpen(w)}
                class="w-full text-left bg-tg-secondary rounded-lg p-3 hover:bg-opacity-70 transition">
          <div class="flex justify-between">
            <span class="font-medium">${w.label || w.product_code}</span>
            <span class="text-tg-hint text-sm">${w.category_prefix}</span>
          </div>
          <div class="text-xs text-tg-hint mt-1">tap for chart →</div>
        </button>`)}
    </div>`;
}

function CatalogBrowser({ booking, catalog, onAdd, onRefreshCatalog }) {
  const [refreshing, setRefreshing] = useState(false);
  const [refreshLabel, setRefreshLabel] = useState("↻ Refresh catalog");
  const [search, setSearch] = useState("");
  const [openCategory, setOpenCategory] = useState(null);

  const categories = useMemo(() => {
    const m = new Map();
    for (const p of catalog) {
      const c = m.get(p.category_id) || { id: p.category_id, name: p.category_name, n: 0 };
      c.n += 1; m.set(p.category_id, c);
    }
    return [...m.values()].sort((a, b) => a.name.localeCompare(b.name));
  }, [catalog]);

  const searchHits = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (q.length < 2) return null;
    return catalog
      .filter((p) =>
        p.title.toLowerCase().includes(q) ||
        (p.category_name || "").toLowerCase().includes(q))
      .slice(0, 30);
  }, [search, catalog]);

  const productsInCategory = useMemo(
    () => openCategory ? catalog.filter((p) => p.category_id === openCategory) : [],
    [openCategory, catalog],
  );

  async function refresh() {
    setRefreshing(true);
    setRefreshLabel("↻ fetching…");
    try {
      const r = await api("/api/catalog/refresh", {
        method: "POST",
        body: { reservation_id: booking.reservation_id },
      });
      tg?.HapticFeedback?.notificationOccurred?.("success");
      setRefreshLabel(`↻ ${r.products_persisted} product${r.products_persisted === 1 ? "" : "s"}`);
      await onRefreshCatalog();
    } catch (e) {
      tg?.HapticFeedback?.notificationOccurred?.("error");
      setRefreshLabel("↻ failed");
    } finally {
      setRefreshing(false);
      setTimeout(() => setRefreshLabel("↻ Refresh catalog"), 2500);
    }
  }

  const productCard = (r) => html`
    <button key=${r.product_code} onClick=${() => onAdd(r)}
            class="w-full text-left bg-tg-secondary rounded-lg p-3 hover:bg-opacity-70">
      <div class="font-medium">${r.title}</div>
      <div class="text-xs text-tg-hint">${r.category_name} · ${priceText(r)}</div>
    </button>`;

  let body;
  if (searchHits) {
    body = searchHits.length === 0
      ? html`<p class="text-sm text-tg-hint">No matches.</p>`
      : html`<div class="space-y-2">${searchHits.map(productCard)}</div>`;
  } else if (openCategory) {
    body = html`
      <div class="space-y-2">
        <button onClick=${() => setOpenCategory(null)} class="text-tg-link text-sm">← All categories</button>
        ${productsInCategory.map(productCard)}
      </div>`;
  } else if (categories.length === 0) {
    body = html`<p class="text-sm text-tg-hint">No products cached yet. Tap <em>↻ Refresh catalog</em>.</p>`;
  } else {
    body = html`
      <div class="grid grid-cols-2 gap-2">
        ${categories.map((c) => html`
          <button key=${c.id} onClick=${() => setOpenCategory(c.id)}
                  class="bg-tg-secondary rounded-lg p-3 text-left">
            <div class="font-medium text-sm">${c.name || c.id}</div>
            <div class="text-xs text-tg-hint">${c.n} product${c.n === 1 ? "" : "s"}</div>
          </button>`)}
      </div>`;
  }

  return html`
    <div class="space-y-2">
      <div class="flex items-center justify-between">
        <h3 class="text-sm uppercase tracking-wide text-tg-hint">Add a product</h3>
        <button onClick=${refresh} disabled=${refreshing} class="text-tg-link text-sm">${refreshLabel}</button>
      </div>
      <input type="search" placeholder="Search drinks, WiFi, excursions…" value=${search}
             onInput=${(e) => setSearch(e.target.value)}
             class="w-full bg-tg-secondary text-tg-text rounded-lg px-3 py-2 placeholder:text-tg-hint focus:outline-none focus:ring-2 focus:ring-tg-button" />
      ${body}
    </div>`;
}

function BookingView({ booking, watched, catalog, onBack, onOpenWatched, refreshWatched, refreshCatalog }) {
  useBackButton(onBack, [booking.reservation_id]);

  async function addWatched(r) {
    try {
      await api("/api/watched", {
        method: "POST",
        body: {
          reservation_id: r.reservation_id,
          category_prefix: r.category_id,
          product_code: r.product_code,
          label: r.title,
        },
      });
      tg?.HapticFeedback?.notificationOccurred?.("success");
      await refreshWatched();
    } catch (e) {
      tg?.HapticFeedback?.notificationOccurred?.("error");
      alert(`Failed: ${e.message}`);
    }
  }

  return html`
    <section class="space-y-4">
      <button onClick=${onBack} class="text-tg-link text-sm">← All bookings</button>
      <${BookingHeader} booking=${booking} />
      <${WatchedList} watched=${watched} booking=${booking} onOpen=${onOpenWatched} />
      <${CatalogBrowser} booking=${booking} catalog=${catalog}
                         onAdd=${addWatched} onRefreshCatalog=${refreshCatalog} />
    </section>`;
}

function ProductDetail({ watched, onBack }) {
  const [points, setPoints] = useState(null);
  const [error, setError] = useState(null);
  const canvasRef = useRef(null);
  const chartRef = useRef(null);

  useBackButton(onBack, [watched?.id]);

  useEffect(() => {
    let alive = true;
    api(`/api/history/${watched.id}`)
      .then((p) => alive && setPoints(p))
      .catch((e) => alive && setError(e.message));
    return () => { alive = false; };
  }, [watched.id]);

  useEffect(() => {
    if (!points || points.length === 0 || !canvasRef.current) return;
    const ordered = [...points].reverse(); // oldest → newest
    const data = ordered.map((p) => ({
      x: p.fetched_at,                     // ISO timestamp; Chart's time axis would parse, but we use category
      y: p.adult_promo_price,
    }));
    const labels = ordered.map((p) => p.fetched_at);
    const accent = tgColor("--tg-theme-button-color", "#2481cc");
    const text   = tgColor("--tg-theme-text-color", "#000");
    const hint   = tgColor("--tg-theme-hint-color", "#999");
    const bg2    = tgColor("--tg-theme-secondary-bg-color", "#f1f1f1");

    chartRef.current?.destroy();
    chartRef.current = new Chart(canvasRef.current, {
      type: "line",
      data: {
        labels,
        datasets: [{
          label: "Adult promo",
          data: data.map((d) => d.y),
          tension: 0.25,
          borderColor: accent,
          backgroundColor: accent,
          pointRadius: 4,
          pointHoverRadius: 7,
          pointBackgroundColor: accent,
          pointBorderColor: bg2,
          pointBorderWidth: 2,
          fill: false,
        }],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        interaction: { mode: "index", intersect: false },
        plugins: {
          legend: { display: false },
          tooltip: {
            backgroundColor: bg2,
            titleColor: text,
            bodyColor: text,
            borderColor: hint + "55",
            borderWidth: 1,
            padding: 10,
            displayColors: false,
            callbacks: {
              title: (ctxs) => fmtDateTime(ctxs[0].label),
              label: (ctx) => {
                const v = ctx.parsed.y;
                if (v == null) return "no price";
                const idx = ctx.dataIndex;
                const prev = idx > 0 ? data[idx - 1].y : null;
                const delta = prev != null && v != null ? v - prev : null;
                const sign = delta == null ? "" : (delta > 0 ? " ▲" : (delta < 0 ? " ▼" : ""));
                const dtxt = delta == null ? "" : ` (${delta > 0 ? "+" : ""}${delta.toFixed(2)})`;
                return `$${v.toFixed(2)}${sign}${dtxt}`;
              },
            },
          },
        },
        scales: {
          x: {
            ticks: {
              color: hint,
              maxRotation: 0,
              autoSkipPadding: 16,
              callback: function (value) {
                return fmtDateShort(this.getLabelForValue(value));
              },
            },
            grid: { display: false },
          },
          y: {
            beginAtZero: false,
            ticks: {
              color: hint,
              callback: (v) => `$${v}`,
            },
            grid: { color: hint + "26" }, // ~15% alpha
          },
        },
      },
    });
    return () => chartRef.current?.destroy();
  }, [points]);

  let meta = "loading…";
  if (error) meta = `error: ${error}`;
  else if (points && points.length === 0) meta = "No price history yet — wait for the daily scrape.";
  else if (points) {
    const data = points.map((p) => p.adult_promo_price).filter((x) => x != null);
    const last = data[0];
    const min = Math.min(...data);
    const max = Math.max(...data);
    meta = `${points.length} snapshots · last ${last?.toFixed(2)} · low ${min?.toFixed(2)} · high ${max?.toFixed(2)}`;
  }

  return html`
    <section class="space-y-3">
      <button onClick=${onBack} class="text-tg-link text-sm">← Back</button>
      <h2 class="text-lg font-medium">${watched.label || watched.product_code}</h2>
      <p class="text-sm text-tg-hint">${meta}</p>
      <div class="bg-tg-secondary rounded-lg p-3">
        <canvas ref=${canvasRef} height="220"></canvas>
      </div>
    </section>`;
}

// ─── root ────────────────────────────────────────────────────────────────────

function App() {
  const [user, setUser] = useState(null);
  const [view, setView] = useState("loading");
  const [bookings, setBookings] = useState([]);
  const [watched, setWatched] = useState([]);
  const [catalog, setCatalog] = useState([]);
  const [selectedBooking, setSelectedBooking] = useState(null);
  const [selectedWatched, setSelectedWatched] = useState(null);

  const loadMe = useCallback(async () => {
    try {
      const me = await api("/api/me");
      setUser(me);
      return true;
    } catch (e) {
      if (/^403/.test(e.message)) { setUser(null); return false; }
      console.error(e);
      return false;
    }
  }, []);

  const loadBookings = useCallback(async () => {
    setBookings(await api("/api/bookings"));
  }, []);

  const loadWatched = useCallback(async () => {
    setWatched(await api("/api/watched"));
  }, []);

  const loadCatalog = useCallback(async (booking) => {
    setCatalog(await api(`/api/catalog/browse?reservation_id=${encodeURIComponent(booking.reservation_id)}`));
  }, []);

  const bootstrap = useCallback(async () => {
    const ok = await loadMe();
    if (!ok) { setView("register"); return; }
    await Promise.all([loadBookings(), loadWatched()]);
    setView("bookings");
  }, [loadMe, loadBookings, loadWatched]);

  useEffect(() => { bootstrap(); }, [bootstrap]);

  async function openBooking(b) {
    setSelectedBooking(b);
    setView("booking");
    await loadCatalog(b);
  }

  let body;
  if (view === "loading") {
    body = html`<p class="text-sm text-tg-hint">loading…</p>`;
  } else if (view === "register") {
    body = html`<${RegisterForm} onRegistered=${bootstrap} />`;
  } else if (view === "bookings") {
    body = html`<${BookingsList} bookings=${bookings}
                                  onOpen=${openBooking}
                                  onRefresh=${loadBookings} />`;
  } else if (view === "booking" && selectedBooking) {
    body = html`<${BookingView}
        booking=${selectedBooking}
        watched=${watched}
        catalog=${catalog}
        onBack=${() => { setSelectedBooking(null); setView("bookings"); }}
        onOpenWatched=${(w) => { setSelectedWatched(w); setView("detail"); }}
        refreshWatched=${loadWatched}
        refreshCatalog=${() => loadCatalog(selectedBooking)}
      />`;
  } else if (view === "detail" && selectedWatched) {
    body = html`<${ProductDetail} watched=${selectedWatched}
                                   onBack=${() => setView("booking")} />`;
  }

  return html`
    <div class="max-w-2xl mx-auto p-4 space-y-4">
      <${Header} user=${user} />
      ${body}
    </div>`;
}

render(html`<${App} />`, document.getElementById("app"));
