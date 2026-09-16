# DeltOS

Visual & UX Design Language

*Working draft · September 2026 · cross-cutting design reference, informed by a competitive survey of comparable platforms (§20) and a product-direction amendment on Guest access (§21)*

## 1. Purpose & Scope

The eight tracks (A–H) settle what DeltOS is made of and how the pieces fit together; none of them specify what it actually looks and feels like to touch. This document is that missing layer — a single, shared visual and interaction language every screen in the system draws from, whether it's a student picking a profile at the minimum spec or an administrator scanning forty devices' health in a Community Hub console. It doesn't introduce new components or daemons — every pattern here is implemented by a track already named in Tracks C, F and G; this document says what those implementations should look like and why, and cross-references the owning track/section for each.

**DeltOS is one WebOS with one light visual language, admin through learner.** There is no dark theme and no separate admin look. High contrast is an accessibility setting, not a second theme.

Two constraints shape every decision below more than aesthetic preference does. First, **the rendering client, not the box**: visitors' own phones and laptops render these pages, so what a design can afford is decided per client, at the client — and the floor a box itself must drive is the minimum spec of the cross-track contract §2.1, not a retired 512 MB board. Second, **offline-first**: no font, icon or asset in this language may load from a CDN or the open internet at runtime (matching the project's no-internet-dependency posture, Track E §21) — everything ships in the image or the pack.

## 2. Design Principles

- **Layered by default, flat as the fallback.** The default look is frosted, translucent panels layered over a soft ambient ground. It falls back to flat — **the same layout**, solid surfaces, no blur — and the two must be indistinguishable in structure, differing only in surface treatment.
  - **The box never decides this. The rendering client does.** The phone or laptop drawing the page picks the fallback when it cannot render blur or translucency acceptably, and it **honours that client's own `prefers-reduced-transparency` and `prefers-reduced-motion` settings**. A viewer who has asked their device for less motion or less transparency gets that answer from their own device, not from a server guessing on their behalf.
  - This inverts an earlier principle in this document, "flat over layered", which justified flatness as a rendering-cost decision at the lowest hardware tier. That tier is retired, and the cost was never the box's to pay in the first place — the client renders.
- **One accent, used sparingly:** A single accent hue carries every "this is active/important/actionable" signal — an underline, a filled tile, a status dot — against an otherwise neutral, warm-toned palette (§4). Restraint is what makes the accent mean something; a screen with five competing colours has no hierarchy left to signal with a sixth.
- **The high-tech feel comes from the data, not from the chrome.** Live graphs, flow lines and dense data drawn in the light palette are what make this read as real infrastructure. The interface around them stays simple and intuitive; the sophistication belongs to what is being shown.
- **Single-focus for the person being taught; multi-pane for the person administering:** A kiosk launcher (C1) and an admin console (F1) are not the same kind of interface and shouldn't share a layout metaphor. The student/teacher-facing shell is single-app, full-screen, no overlapping windows — Track C's settled model, closer to Sugar's activity-first posture (§3) than to a desktop metaphor. The admin and monitoring surfaces (F1, F6, G7) are where a genuine multi-pane dashboard layout earns its place, because an administrator really is coordinating several things at once. **They differ in layout, never in palette or theme.**
- **Calm institutional tone, not playful chrome:** Warm neutrals, clean geometric type, minimal ornamentation — a tone that reads as trustworthy, considered infrastructure for a school or library, not a consumer app chasing engagement.
- **Every screen traceable to one owning track:** Nothing here exists free-floating — §17's ownership map is the enforcement mechanism. A new screen fitting no track's scope is a gap to raise with the owning track, not licence for this document to expand scope no track agreed to build.

## 3. Competitive & Reference Survey

Six comparable systems were reviewed before writing this document's actual specifications — three offline/education-focused shells DeltOS competes with or resembles, and three self-hosted admin/fleet consoles relevant to Tracks F and G. The point isn't to imitate any one of them; it's to borrow what real usage has already validated and name, explicitly, where DeltOS deliberately does something different and why.

### Internet-in-a-Box / RACHEL (the direct predecessor)

IIAB's kiosk-facing home page is an auto-generated, flat list of installed content modules with no unified search (each module — Kiwix, Kolibri, Calibre-Web, MediaWiki — has its own separate search box) and no platform-level account system (each module ships its own default, often shared, credentials). Its admin console is a plain checkbox-and-log-viewer forms interface with a non-atomic two-step "Save Configuration" then separately "Install Configured Options" apply flow, and its captive portal is widely reported as broken across recent Android/macOS/Windows versions. DeltOS deliberately does not repeat any of these: C3 unifies search across every installed pack through one endpoint (Track D), F3/F11 gives every profile one real identity rather than per-module logins, F4's install pipeline is atomic and resumable by design (Track F §4), and Track E's E3 rebuild explicitly targets the same captive-portal friction IIAB never fixed.

### ChromeOS (kiosk/education deployment patterns)

ChromeOS's most borrowable idea is consolidating every accessibility feature into one panel rather than scattering vision/hearing/motor settings across unrelated menus (§12 below adopts this directly), and its Quick-Settings pattern of surfacing the highest-frequency toggles first with deeper settings one expansion away. Its least borrowable idea, for an offline-first product, is that almost everything — Family Link's parental controls, its first-boot flow's account provisioning, its ephemeral Managed Guest Session model — is mediated by a live connection to Google's servers; DeltOS has no equivalent identity provider to phone home to; every control here is enforced locally, by identityd (Track F §3), with no cloud round-trip. ChromeOS's own documented classroom pain point — shared-device sign-in being slow and error-prone for young students, and "switch user" really meaning "log out, then log back in as someone else" rather than a true fast switch — is a specific failure this document designs around directly (§9).

### Endless OS and Sugar/OLPC (offline-first education shells)

Endless OS's full-screen icon-grid home (phone-metaphor rather than desktop-metaphor) with one search field that works identically online or offline is close to what C1's launcher should already feel like, and its offline-capable Help Center — tutorials that work with zero connectivity — is a pattern worth matching for DeltOS's own onboarding/help surfaces rather than assuming a help link can point at the open web. Sugar's Journal/Activity model (no file manager, no Save dialog, auto-saved chronological history, one full-screen activity at a time) is the strongest available precedent for a young-learner interaction model, and retrospectives a decade-plus later still credit the idea itself — the recurring criticism of OLPC's broader program was teacher training and deployment support, not the interface paradigm. DeltOS borrows the no-file-management instinct for anything that is a learning activity (C4's sandboxed packs already have no exposed filesystem to a student) without adopting Sugar's full paradigm break (no windows, mesh-neighborhood-as-home-screen) — the retrospective risk that a fully novel paradigm doesn't transfer to the ordinary devices a student will meet everywhere else in life is real, and Track C's shell already looks more like a conventional (if simplified) launcher than a reinvention.

### Kolibri (already embedded as H1)

Because Kolibri is a component DeltOS ships, not just a reference point, consistency with its own UI conventions matters more here than with any other system surveyed. Its resource cards use type-specific iconography (Watch/Listen/Practice/Read/Explore) that §6 generalizes into DeltOS's own content-type badging; its breadcrumb topic navigation with an overflow dropdown is the pattern §7 specifies for C2/C6's own browsing; its passwordless-learner setting is exactly the identity model Track F §21 now adopts for DeltOS-native student profiles, for the same reason Kolibri adopted it; and its own coach-dashboard redesign case study (published usability research, not marketing material) — collapsing six tabs to three mapped to the actual formative-assessment workflow, and replacing raw data with actionable "needs help" signals — is the direct model §16 uses for C8.

### Cockpit, balenaCloud, and Portainer (self-hosted admin/fleet consoles)

Cockpit's Overview page groups a server's status into four fixed quadrants (Health, Usage, Configuration, System information) — a genuinely good one-glance mental model, though its actual navigation (Storage, Networking, Accounts, Services as top-level items) assumes a sysadmin audience DeltOS's admin console does not have; §14 borrows the quadrant structure but not the sysadmin vocabulary. balenaCloud's fleet-of-devices dashboard offers three ideas §15 adopts directly: a device-status vocabulary richer than online/offline (distinguishing degraded, updating, and fully dark states), free-form tags for ad hoc grouping instead of a rigid pre-built hierarchy, and saved/named filter views for a recurring query. Portainer's per-environment summary tile and its checkbox-select-then-bulk-action pattern for applying one action across many resources at once round out §15's fleet-dashboard specification.

## 4. Color System — Locked, AA-Verified Tokens

One light theme, every surface, admin through learner. **These tokens are locked.** The contrast column is evidence from measurement, not a target still to be hit — an earlier revision of this document asserted blanket AA compliance it had never verified, and six of its pairings did not hold.

### Surfaces

| **Token** | **Hex** | **Usage** |
|---|---|---|
| `bg` | `#E9E6DF` | The ambient ground everything sits over. |
| `surface` | `#F2EEE6` | Panels and cards over the ground. |
| `raised` | `#FAF8F4` | The layer above a panel — active tile, popover, floating panel. |
| `line` | `#DDD6C8` | Hairline separators. |
| `line-strong` | `#C7BFAE` | Input outlines, where an edge has to be found rather than merely implied. |

### Ink

| **Token** | **Hex** | **Contrast** | **Usage** |
|---|---|---|---|
| `ink` | `#2B2926` | **12.5:1** | Primary text, titles, icon fill. Near-black, never pure black. |
| `ink-2` | `#6B655C` | **5.0:1** | Secondary text — metadata, timestamps, breadcrumbs, helper text. |
| `ink-3` | `#8E887E` | — | Faint labels. **Decorative only — never body text.** |

### Accent — one hue, two shades

This split is the fix for the pairings that previously failed. One hue was being asked to serve both as a fill and as text, and no single value can do both at AA.

| **Token** | **Hex** | **Contrast** | **Usage** |
|---|---|---|---|
| `accent` | `#C1622A` | **3.6:1** — passes UI ≥3.0 | Fills, the active-tile block, underlines, icon glyphs. |
| `accent-ink` | `#A4460C` | **5.3:1** text · **6.1:1** white on it — passes ≥4.5 | Accent-coloured *text*, links, the primary-button fill under white text, and the focus ring. |

- **`accent` is never text. `accent-ink` is never a large fill behind white below its verified use.** These two rules are what keep the pairings passing; violating either re-creates the original defect.

### State

| **Token** | **Hex** | **Contrast** | **Usage** |
|---|---|---|---|
| `success` | `#3A7143` | **5.0:1** | Completed/healthy only — never decorative. |
| `warn` | `#8A5A00` | **5.1:1** | Needs attention, not yet failed. |
| `error` | `#9E2F2F` | **6.3:1** | Failed/blocked, and destructive-action confirmation. |

- **Every state carries its dot *and* its word.** State never rests on colour alone — that is a correctness rule for colour-blind viewers, not a stylistic preference.

### The three-segment motif

| **Token** | **Hex** | **Usage** |
|---|---|---|
| `charcoal` | `#3A3834` | The dark segment of the rule. |
| `rule-a` | `#A7A298` | The grey segment. |

The rule is grey / charcoal / accent, topping panels and the dock, and **doubles as the progress indicator** — the accent segment's extent is the progress.

### Verification note

The contrast figures above are measured against **`surface` `#F2EEE6`**, the surface each token most often sits on. Measured against the darker `bg` `#E9E6DF`, every ratio is lower but **every token still passes its stated requirement**: `ink` 11.64, `ink-2` 4.63, `accent-ink` 4.87, `success` 4.65, `warn` 4.76, `error` 5.81, `accent` 3.34 (UI, ≥3.0). The headroom on `bg` is thinner than the quoted figures suggest, so **a new token is validated against `bg`, not against `surface`**. `ink-3` measures 2.82 on `bg`, which is why it is restricted to decoration.

## 5. Typography, Spacing and Targets

### 5.1 Type

**Three bundled faces, shipped in the OS image, never fetched at runtime.**

| **Face** | **Role** |
|---|---|
| Display — thin geometric | Names, headers, the clock. **Thin weights at large sizes only** — a thin face at Body size is a legibility failure. |
| Body — readable | Everything read at arm's length. |
| Mono | Data labels, and the small monospace annotations the data overlays use. |

**Five sizes, and only five:** Display 32 · Title 24 · Body 16 · Label 14 · Caption 12.

- **Weight, not size, carries emphasis.** Reaching for a sixth size is a sign a layout needs simplifying, not a scale that needs extending.
- This replaces the previous "system font stack, no bundled faces" decision. Bundling is what makes one visual language hold across a school laptop, an Android phone and a kiosk display, none of which share a system font.

### 5.2 Spacing and radius

- **Spacing: a 4px base scale — 4 · 8 · 12 · 16 · 24 · 32 · 48 · 64.** Every margin, padding and gap is a value on it.
- **Radius: 0 for tiles · 4px for cards, buttons and inputs · 8px for floating panels.** Crisp, never fully rounded.

### 5.3 Focus and touch targets

- **Focus: 2px solid `accent-ink`, 2px offset, on every interactive element. Never removed.** Not "never removed without a replacement" — never removed. At 4.87:1 against `bg` it clears the 3:1 non-text requirement comfortably.
- **Touch targets: 44×44px minimum, 48px preferred.** Above the WCAG AA floor deliberately, and sized for young hands on shared touch devices rather than for the specification's minimum.

## 6. Iconography & Content-Type Badging

- **Flat, single-weight line icons:** No gradients, no skeuomorphic shading, one consistent stroke weight across every icon in the system — icons stay flat and single-weight even though panels are layered (§2): a glyph carries meaning by silhouette, and shading it costs legibility at Caption size for nothing.
- **Content-type badges, generalized from Kolibri's convention:** Every content tile (a WAX pack, a Track H service, a Kolibri resource) carries one small type badge before a person reads its title: Read (text/reference), Watch (video), Listen (audio), Practice (interactive exercise), Explore (an app or tool), and Tool (a Track G/H service that isn't content at all — office suite, chat, email). This mirrors Kolibri's own five-category convention (§3) so a resource looks the same whether it's rendered by Kolibri's own UI or by C2's launcher grid — one visual vocabulary for "what kind of thing is this" across the whole system, not two competing ones.
- **Status icons are a fixed, shared vocabulary:** Exactly the states named in §15's fleet-status table and nowhere else invented ad hoc per screen: a filled accent dot (active/healthy), a filled success-color dot (completed), a hollow/outlined dot (idle/not yet started), a spinning ring (in progress), and a filled error-color dot (failed/needs attention) — reused identically across C8's progress view, F5's device health, F6's fleet dashboard, and G7's metrics.

## 7. Core Component: Persistent Shell Dock

The one piece of chrome visible on every screen of the student/teacher-facing shell (Track C's C1) — a full-width bar anchored to one edge, holding identity plus the highest-frequency actions, modeled on the shared-device dock pattern described in §3 but mapped explicitly onto Track C's own named components rather than an arbitrary icon set.

| **Dock item** | **Owning component** | **Behavior** |
|---|---|---|
| Profile name/avatar | C5 (Track C §6) | Opens the switch-profile screen (§9) — never a settings menu; switching who's using the device is the action, not a side effect of opening settings. |
| Home | C1/C2 (Track C §2, §3) | Returns to the launcher grid from anywhere, including from inside a running pack (C4) or a Track H service. |
| Search | C3 (Track C §4) | Opens the unified search overlay — one field, one ranked result list, matching §3's assumed-contract design. |
| Ask DeltOS | C11 (Track C §11) | Present only where D7's hardware tier permits (§12's hardware-tier absence rule) — omitted from the dock entirely on Kiosk-tier hardware rather than shown disabled. |
| Notifications | New — specified in §11 below | Badge count plus a dropdown list; the transport for Track H's credential-disclosure mechanism (Track H §2) and C2's change-notification pattern (Track C §3) converge on this one UI surface rather than each inventing its own. |
| Coach dashboard | C8 (Track C §8) — teacher/admin roles only | Hidden entirely for a student-role profile, per F3's role model (Track F §3) — not shown-then-blocked. |
| Settings | C9/C10 scope | Language, accessibility (§12), and (admin/teacher only) a link into F1. |
| Lock | identityd session state (Track F §3) | Ends the active profile's session and returns to the switch-profile screen (§9) without a full shell restart. |

## 8. Core Component: Breadcrumb Browser & Card/Tile Grid

- **Breadcrumb trail:** A horizontal path (Category > Subcategory > current) sits above any browsing surface — C2's launcher grid when grouped by category (Track C §3), C6's offline app store, and any Track H service's own content hierarchy that's exposed through the shell rather than that service's native UI. When the trail is too long for the available width, it collapses the middle segments into a single overflow control rather than wrapping to a second line or truncating silently — the same overflow-dropdown behavior Kolibri already uses (§3).
- **Card/tile grid:** Every browsable item renders as a tile: a thumbnail or icon, the content-type badge (§6), a title, and a metadata footer (size, item count, or last-updated, whichever is most relevant to that content type) — reused identically whether the tiles are WAX packs (C2/C6), starter-pack choices (C10's wizard), or Track H service tiles. One grid component, several data sources, never a bespoke layout per source.

## 9. Core Component: Switch-Profile Screen

A tap/click roster of every profile registered on that device (sourced from identityd's list_profiles call, Track F §3), not a text-entry login form — this is the shell-side counterpart Track C §22 and Track F §21 both call for once student profiles became passwordless.

- **Student profiles:** A single tap on that student's tile signs them in immediately — no password step at all, matching Kolibri's own passwordless-learner convention (§3) and directly designed around ChromeOS's documented classroom sign-in friction (§3).
- **Admin/teacher profiles:** Tapping the tile reveals a password field inline (not a full-screen redirect) — the one place in the whole shell a password is asked of anyone, which makes the exception legible: a password prompt on this screen always means "this profile can do something a student's can't."
- **Kiosk-tier absence:** On a Kiosk-profile device running with exactly one implicit profile (Track C §12), this screen is never shown at all — there's nothing to switch between, and showing a one-tile switcher would just be a confusing extra tap on hardware that has no admin console reachable anyway.
- **Guest tile, conditionally present (§21):** When an admin has enabled Guest access (Track F §22), one additional tile appears — labeled "Guest," tap-to-enter like a student tile, no password. It represents one shared identity, not a per-visitor one; nothing about the session it starts is remembered once Lock (§7) ends it.

## 10. Core Component: First-Boot Wizard Shell

One consistent wizard chrome (a progress indicator showing step N of the total, a Back/Next pattern, no dead ends) wraps every screen in C10's onboarding flow, in the order settled by Track C §22's amendment:

| **Step** | **Screen** | **Notes** |
|---|---|---|
| 1 | Admin password (F2, Track F §2) | Generated password shown once, plaintext and QR code, with an explicit confirmation before continuing. |
| 2 | Language & accessibility (Track C §22) | Asked before network setup specifically so the wizard's own remaining screens can already apply the chosen language/TTS/contrast settings. |
| 3 | Network setup | Wi-Fi credentials or confirmation of no network available — the wizard must be able to complete fully offline past this point. |
| 4 | Deployment Profile selection | Kiosk/Classroom/Community Hub/Field Ops, per the master plan §2 — sets sensible defaults for the remaining steps, never locks a later choice out. |
| 5 | Starter-pack selection | Track B §7's B5 curated bundles, presented as the card/tile grid (§8). |
| 6 | First profile creation | C5 — the very first non-admin profile, using the same switch-profile-screen conventions (§9) it will use forever after. |

## 11. Core Component: Notification & Toast Pattern

- **One shared surface, two producers so far:** The dock's notification item (§7) is the single UI surface for both C2's change-notification pattern (a pack install completes, a catalog sync lands new metadata — Track C §3) and Track H's credential-disclosure mechanism (identityd flags a pending disclosure; the shell polls and clears it — Track H §2). A third producer joining later (Track G/H service registering itself, an F5 health alert reaching a teacher) uses the same surface rather than inventing its own toast/banner convention.
- **Toast for the moment, list for the history:** A transient toast (auto-dismissing, non-blocking) announces a notification as it arrives; the dock's dropdown keeps a short persistent log of recent ones so a notification missed in the moment isn't lost the way a ChromeOS notification effectively is once dismissed (§3's noted gap in that system) — bounded to a reasonable count (e.g. the last 20), not an unbounded history.

## 12. Core Component: Consolidated Accessibility Panel

One settings screen, not accessibility options scattered across unrelated menus — directly modeled on the single strongest pattern the ChromeOS survey surfaced (§3). Reachable from the dock's Settings item (§7) and, on first boot, from C10's dedicated step (§10).

| **Feature** | **Notes** |
|---|---|
| Text-to-speech | Piper, tier-gated per Track C §9 — ships on every tier as a baseline feature, footprint permitting. |
| Translation | Tiered per D7's hardware gating (Track C §9) — likely out of scope on Kiosk-tier hardware. |
| High-contrast / large text | Shell-level presentation only; content inside a pack's sandboxed iframe is handled through reader mode below, not direct restyling (Track C §9's stated C4 boundary). |
| Reader mode | Extracts a declaring pack's own content via its postMessage export (Track C §9) and re-renders it in a shell-controlled, accessibility-styled view. |
| Magnifier | New in Track C §22 — a docked mode (magnifies a screen region, leaving the rest live, matching the pattern named in the ChromeOS survey) and a full-screen mode, implemented as deltos-shell presentation-layer zoom. |

## 13. Core Component: Data-Overlay Widget Kit

One small set of shared widgets for presenting live or historical numeric data, reused wherever a screen needs to show status/progress rather than each surface inventing its own chart style — C8's coach dashboard, F5's device health, F6's fleet dashboard, and G7's metrics stack all draw from this same kit.

- **Progress bar:** A horizontal filled bar plus a fraction/percentage label — used for anything with a clear completion state (a lesson, a download, a pack install).
- **Sparkline:** A small, axis-less line showing a recent trend (CPU/memory over the last hour, say) — deliberately minimal, meant to answer "is this climbing or stable," not to be read precisely; a person who needs exact values clicks through to G7's fuller history (§14's progressive-disclosure principle).
- **Status dot + label:** The fixed vocabulary from §6 — never a bespoke color or icon invented per screen.
- **Stat tile:** A single large number with a short label beneath it (device count, storage used, active profiles) — the building block of the quadrant layout in §14.

## 14. Admin & Fleet Console Shell (F1)

F1's admin console (Track F §5) is the one screen in the system that earns a genuine multi-pane, dashboard layout per §2's principle. Its home screen borrows Cockpit's four-quadrant Overview structure (§3) but translates the vocabulary from sysadmin language into the plain-task language a librarian or teacher-administrator actually thinks in.

| **Quadrant** | **Answers, in plain language** |
|---|---|
| Is everything OK? | Cockpit's "Health" quadrant, renamed — aggregates F5's device-health signal and (once Track G lands) G1's per-service health (Track F §19) into one pass/fail-with-detail summary, using the status-dot vocabulary (§6). |
| How busy is it? | Cockpit's "Usage" quadrant — storage, active profiles, and (Community Hub tier) service load, as stat tiles (§13) with a sparkline where a trend is meaningful. |
| What's set up? | Cockpit's "Configuration" quadrant — Deployment Profile, installed Track H services, fleet-pairing status (F6) — a glance at what this box is, not a settings-editing surface itself. |
| What is this box? | Cockpit's "System information" quadrant — hardware tier, hostname, DeltOS version — pushed to a corner rather than default-visible clutter, matching Cockpit's own choice to de-emphasize it (§3). |

- **Progressive disclosure of monitoring depth:** The Overview quadrants work with zero configuration, showing current-reading data only; a single, clearly-labeled action unlocks G7's fuller historical graphs where G7 is actually installed (Community Hub tier, Track G §7) — mirroring Cockpit's own Metrics page pattern (§3) rather than cluttering F1's default screen with a monitoring stack most Kiosk/Classroom deployments will never run.

## 15. Fleet Dashboard Specifics (F6)

Track F §21 confirms this is where F6's own dashboard UI — deliberately left unspecified in Track F's own document — actually gets designed, informed directly by the balenaCloud and Portainer patterns surveyed in §3.

- **Status vocabulary, richer than online/offline:** Operational, Degraded (reachable but something's off — e.g. F5 health flags an issue), Updating (mid-install via F4), and Offline (not reachable at all) — four states, not two, so an admin can tell "it's alive but needs attention" apart from "it's completely gone" at a glance.
- **Free-form tags for grouping:** An admin attaches arbitrary key/value tags to any device (by building, by room, by grade level) rather than being forced into one rigid pre-built hierarchy — the same ad hoc grouping mechanism balenaCloud offers (§3), scoped to whatever grouping actually matters for that organization's fleet.
- **Saved filter views:** A filter (by tag, by status, by Deployment Profile) can be saved with a name and recalled with one click — "devices needing attention" becomes a standing view rather than a query rebuilt from scratch every visit.
- **Checkbox-select, then bulk action:** Selecting several devices surfaces a contextual action bar (push an update through F4, restart a service, re-run F5's health check) — the same select-then-bulk-act interaction pattern used by both balenaCloud's device table and Portainer's container table (§3), so one admin can manage a fleet of forty kiosks without forty individual clicks.

## 16. Coach / Progress Dashboard (C8)

Modeled directly on Kolibri's own published coach-dashboard redesign (§3) — not a generic admin table, because Kolibri's usability research already validated this specific shape with real teachers, and C8 sits right next to Kolibri (H1) in the same shell, so keeping their layouts conceptually aligned matters for a teacher who uses both.

- **Three tabs, mapped to the formative-assessment cycle:** Class Home (what's happening right now), Reports (progress by learner or by resource), and Plan (what to assign next) — collapsed from a longer, harder-to-navigate tab set the same way Kolibri's own redesign collapsed six tabs to three.
- **Actionable signals, not raw tables:** A "needs help" indicator surfaces automatically wherever a learner is stuck or a resource has an unusually high failure rate — the dashboard's job is to point a teacher at the one thing worth acting on today, not to hand over a spreadsheet and let them find it themselves.
- **Natural-language activity feed:** Recent events narrated in plain sentences ("Maria and 2 others completed the Fractions quiz") rather than a timestamped log table — each entry clickable through to that learner's or resource's own detail, matching Kolibri's own Class Activity feed.
- **Multi-source progress, one dashboard:** C8 reads from progress_events regardless of event_source (Track F §20) — a WAX-pack completion and a Kolibri lesson completion render in the same feed, distinguished by the content-type badge (§6), rather than a teacher needing to check two separate dashboards for two kinds of content.

## 17. Screen-by-Track Ownership Map

| **Screen / surface** | **Owning track** | **This document's section** |
|---|---|---|
| Launcher grid, search overlay | Track C (C1-C3) | §7, §8 |
| Per-pack sandboxed content | Track C (C4) | Not restyled by this document — see Track C §9's stated C4 boundary |
| Switch-profile screen | Track C (C5), Track F (identityd) | §9 |
| Offline app store / breadcrumb browsing | Track C (C6, C2) | §8 |
| Coach / progress dashboard | Track C (C8) | §16 |
| Accessibility panel | Track C (C9) | §12 |
| First-boot wizard | Track C (C10), Track F (F2) | §10 |
| Ask DeltOS | Track C (C11) | §7 (dock entry only — its own conversational UI is out of this document's scope) |
| Admin console home | Track F (F1) | §14 |
| Fleet dashboard | Track F (F6) | §15 |
| Metrics / historical graphs | Track G (G7) | §13, §14 |
| Notification surface | Track C (dock, §7) + producers in Track C/H | §11 |

## 18. Accessibility Summary

- Every colour pairing in §4 is **measured** against WCAG 2.2 AA at its specified use, on both `surface` and the darker `bg`, and the ratios are recorded in §4 as evidence. A new token is validated against `bg`. Composition-level validation against rendered mockups remains listed in §19.
- §12 consolidates every accessibility feature into one panel, reachable identically from the dock and from first-boot onboarding — never a feature a person has to already know exists to find.
- §9's passwordless student flow and §7's role-gated dock items exist partly as accessibility decisions in their own right — a shared classroom device should not require reading and typing a password to become usable for a student who can't yet do either reliably.
- Nothing in this document assumes an always-on internet connection to render, translate, or narrate — TTS (Piper) and translation both run locally per Track C §9, matching this document's own §1 offline-first constraint.

## 19. Open Decisions Needed Before Implementation

- **The exact icon set** — a specific icon library or a custom-drawn set — implementing §6's flat, single-weight style. This document specifies the visual rule, not the asset source, and **nothing in §4's locked tokens depends on the answer.** Listed as open in cross-track contract §13.2.
- Composition-level WCAG validation against real rendered mockups. §4's token pairings are measured and recorded; what measurement cannot pre-empt is a specific composition — small Label text over a tinted card, an accent glyph on `raised` inside a translucent panel — where the effective background is not the token it nominally sits on. This checks compositions, not tokens.

**Closed since the previous revision:**

- *Design-system accessibility values* — the palette, type scale, spacing, radius, focus indicator and touch-target minimum are settled and recorded in §4 and §5. This was blocking decision 8 in cross-track contract §13; it is closed there too.
- *Whether a bundled fallback font is necessary* — settled the other way. §5.1 bundles **three** faces and does not use a system stack at all, because one visual language cannot hold across a school laptop, an Android phone and a kiosk display when each supplies a different default.
- *The walk-in-visitor question* raised in Track C §22 — settled by the product owner; see §21.

## 20. Competitive Gap-Check — Findings and Resolutions

Researching this document meant surveying six comparable systems (§3) closely enough to check DeltOS's existing eight tracks against real, field-tested patterns those systems already validated. That check surfaced a small number of genuine architecture-level gaps — not settled-vs-settled contradictions like the earlier per-track and holistic reviews, but real omissions no prior review had reason to catch, since it took an outside reference point to notice them. Each was resolved directly in its owning track's document, not just described here.

- **[Critical]** Neither Track F nor Track C specified how a student profile actually gets created or authenticates — F3/F11's design (Track F §3) covers admin and teacher profile creation explicitly and silently assumed students worked the same way. Given Kolibri (already embedded as H1) treats passwordless learner sign-in as a deliberate, validated design choice, and ChromeOS's own classroom deployments cite password-based shared-device sign-in as one of their most consistently reported sources of friction, this was a real gap, not a stylistic preference. Resolved: password_hash is now nullable for student-role profiles (Track F §21); a shell-side switch-profile screen (§9 above) is specified as the concrete student-facing counterpart.
- **[Moderate]** C10's onboarding wizard (Track C §10) never included a language or accessibility-preference step, despite C9 defining real accessibility features a user would want active from their first screen. ChromeOS's own first-boot flow asks language/accessibility before anything else for exactly this reason. Resolved: a language-and-accessibility step added as C10's second screen, before network setup (Track C §22, §10 above).
- **[Moderate]** C9's accessibility layer covered TTS, translation, and high-contrast/reader-mode thoroughly but had no screen-magnification feature — a low-cost, well-precedented accessibility pattern present on every major platform surveyed. Resolved: a docked and full-screen magnifier added to C9's scope (Track C §22, §12 above).
- **[Minor]** Track F's F6 (fleet dashboard) explicitly deferred its own dashboard UI as out of scope for that document ("that's most of this section's real content, not the dashboard UI itself") — not an error, but a deliberately open question this document was the right place to close. Resolved: §15 above specifies the actual dashboard (status vocabulary, tags, saved views, bulk actions), confirmed back into Track F §21.

## 21. Amendment (per product direction on Guest access)

The product owner settled §19's walk-in-visitor question directly: every user gets a real profile — DeltOS has no fully anonymous access path — but an admin may opt in to a single shared Guest profile with narrow, curated access, matching a familiar library or school computer-lab guest login. Confirmed in Track F §22 (the role/permission model), Track C §23 (the shell-side surface), Track B §2 (the manifest flag that scopes what Guest can see), and Track H §18 (excluding Guest from the Identity Bridge). §9 above already reflects the resulting switch-profile-screen behavior; this section is the design record of the decision itself.

- **Why a shared identity, not a per-visitor one:** A named, per-person profile is the right model for anyone whose progress or files matter across sessions (§16's whole reason for existing); a walk-in visitor has neither. Modeling Guest as one shared, admin-toggled identity — rather than inventing lightweight per-visitor accounts — keeps the system's identity model to exactly two shapes (a real profile, or the one shared Guest profile), not three.
- **Visually, Guest is deliberately unremarkable:** No distinct color, badge, or special chrome marks a Guest session as such beyond the tile it was entered from — a smaller launcher grid (filtered to guest_accessible content, Track B §2) is the only visible difference from any other profile's experience. A student or teacher shouldn't need to think about who's using the box next to them; a design that made Guest visually distinct (a warning color, a persistent banner) would frame ordinary public-computer access as suspect, which isn't the tone §2's principles call for.

## 22. Amendment (per product direction on the visual language)

Settled direction, recorded here because it changes what §7 and §8 render rather than which track owns them. §4 and §5 hold the tokens; this section holds the form.

### 22.1 Surfaces

- **Frosted, layered panels over a soft blurred backdrop.** The ambient ground is `bg`; panels are `surface` with translucency and blur over it; the layer above a panel is `raised`. The **flat fallback keeps the identical layout** and substitutes solid `surface` and `raised` with a `line` edge, per §2 — and the **client** decides which it draws, honouring its own reduced-transparency and reduced-motion settings.
- **The three-segment rule** — `rule-a` grey, `charcoal`, `accent` — tops panels and the dock, and **doubles as the progress indicator**: the accent segment's extent is the progress. One motif serving both jobs is why progress never needs a separate bar competing for attention.

### 22.2 The launcher (C1)

- **A mosaic of square tiles in varied sizes**, not a uniform grid. Size carries editorial weight — what matters most is bigger — and the varied mosaic is what stops a wall of packs reading as an undifferentiated list.
- **Accent-coloured tiles mark what is new or active**, using `accent` as a fill (never as text — §4).
- Tile radius is **0** (§5.2). Tiles are the one element in the system with square corners, which is what makes the mosaic read as a mosaic.

### 22.3 The dock (§7)

- **A labelled dock along the bottom edge** — labels, not icons alone. An unlabelled icon row is a guessing game for a learner meeting the system for the first time, and labels cost one Label-size line.
- Topped by the same three-segment rule.

### 22.4 Navigation and window furniture

- **Chevron breadcrumbs** for the browse trail (§8), in `ink-2`, with the current level in `ink`.
- **Hexagonal window controls** on the admin surfaces' panes — the one deliberately distinctive piece of chrome in the system, and the reason it is allowed is that it appears on the multi-pane admin layout only, where a pane genuinely needs controls.
- **Small monospace data labels** (§5.1's mono face, Caption or Label size) for every number, identifier, rate and timestamp — the annotation style that makes the data overlays of §13 read as instrumentation rather than as decoration.

### 22.5 What this does not change

The ownership map in §17 is unaffected: every surface named here is still rendered by the track that already owned it. This amendment changes appearance, not ownership.
