# App Review Notes — Tabular for iPadOS / iOS

Source of truth for what we tell Apple App Review. Paste the short version into
**App Store Connect → App Review Information → Notes**, and send the long
version as the reply to a Guideline 2.1 information request.

Placeholders written as `<FILL IN>` must be filled before submitting.

- **App name:** Tabular
- **Bundle ID:** `id.tabular.database` (from `Tabular.xcodeproj`, scheme
  `Tabular-iOS` — the project `apple/scripts/publish_xcode.sh` archives. The
  separate `ios/Xcode/TabulariOS/` project is not what ships.)
- **Category:** Developer Tools
- **In-App Purchases:** none — the app has no StoreKit code and no paid tier
- **Ads / tracking / analytics SDKs:** none

---

## 1. Screen recording

Record on a physical iPad running the current iOS, starting from tapping the
app icon. Cover, in order:

1. Cold launch to the main window.
2. Tap **Sample Database** in the sidebar — a bundled SQLite database opens
   with no sign-in and no file picker required.
3. Expand a table, run `SELECT * FROM customers LIMIT 20;`, show the grid.
4. Edit a cell and save, to show write support.
5. Open **Settings → Sync & Account** and point out the notice that an account
   is optional.
6. Sign in with Apple. Show the account screen once signed in.
7. Open **Teams**, expand a team's **Members**, and show the flag (report) and
   block buttons on a member row. Open the report form so the reasons are
   visible, then cancel.
8. Scroll to **Danger Zone → Delete Account**, type the email to confirm, and
   complete the deletion. Show that the app returns to its signed-out state and
   still works.

Steps 7 and 8 are the two App Review specifically asks about, so do not cut
them short.

---

## 1b. User-generated content, reporting and blocking

The only place one user's content reaches another is **Teams**: a team name,
its description, and the folder names shared inside it are visible to the
people in that team. There is no public feed, no profile browsing, no
messaging, and no way to discover another user without already knowing their
exact email, username, or phone number.

Because being added to a team does not require the invitee's consent, all three
Guideline 1.2 mechanisms are in the app:

- **Report** — a flag button on every team member row opens a report form with
  reasons (abusive content, harassment, spam, illegal content, other) and a
  free-text field. Reports are recorded server-side and reviewed.
- **Block** — a block button on the same row. Blocking removes both people from
  each other's teams immediately, stops any future invitation, and hides each
  from the other in member search. Blocks are managed and undone at
  Settings → Sync & Account → Blocked Users.
- **Leave** — any member can remove themselves from a team they were added to.

Where to find them in the recording: sign in, open Teams in the sidebar, expand
a team's Members list. The flag and block buttons sit on each member row.

Contact for content concerns: `<FILL IN support email>`.

---

## 2. Purpose and target audience

Tabular is a database client and SQL editor. It connects to a database the user
already owns, browses its schema, runs SQL, and shows results in an editable
grid.

**Audience:** software developers, database administrators, and data analysts.

**Problem it solves:** desktop-class database clients have no usable iPad
equivalent, so this audience cannot inspect or fix a database from a tablet.
Tabular gives them one app that speaks all the engines they use, instead of
juggling one vendor tool per database.

**Supported engines:** MySQL, PostgreSQL, SQLite, Microsoft SQL Server,
MongoDB, Redis.

**Value:** offline-first. Every feature except cloud sync works with no account
and no network beyond the user's own database.

---

## 3. Setting up and reaching the main features

**No login is required, and no credentials are needed to review the app.**
The account exists only for optional cross-device sync.

Fastest path for a reviewer:

1. Launch the app.
2. In the sidebar, tap **Sample Database**. This is a small SQLite database
   bundled inside the app; it needs no setup, no file import, and no network.
3. Expand it to see tables, tap one to browse rows.
4. Open a query tab and run any SQL, for example `SELECT * FROM customers;`.

To review the optional account features instead:

- **Sign in with Apple** is available on the sign-in screen.
- Demo account, if one is preferred: `<FILL IN email>` / `<FILL IN password>`
- There is only one account type. There are no roles or paid tiers.
- **Account deletion** is at Settings → Sync & Account → Danger Zone → Delete
  Account. It deletes the account on the server immediately; it is not a
  request form and not a link to a website.

---

## 4. External services used

| Service | Purpose | Required? |
|---|---|---|
| `api.tabular.id` (our own server) | Optional account, cross-device sync of connections/queries/history, team sharing | No — only when signed in |
| Sign in with Apple | Authentication for that account | No |
| Google OAuth / GitHub OAuth | Alternative authentication for that account | No |
| The user's own database servers | The core function of the app; hosts are entered by the user | Yes, for the user's own data |
| OpenAI, Anthropic, Groq, GitHub Models, or a user-specified OpenAI-compatible endpoint | Optional AI assistant for writing SQL | No — inactive unless the user supplies their own API key |

Notes:

- The AI assistant is **bring-your-own-key**. We ship no key, and no request is
  made to any AI provider until the user enters one.
- Synced credentials are end-to-end encrypted on the device (AES-GCM, with the
  key derived from a passphrase via Argon2id) before upload. Our server stores
  ciphertext it cannot read.
- No analytics, advertising, attribution, or crash-reporting SDK is present.
- The iOS build contains no self-update mechanism; the App Store is the only
  update channel.

---

## 5. Regional differences

None. The app ships the same features, the same content, and the same price
(free) in every region. There is no geographic gating, no region-specific
content, and no region-dependent behaviour. The interface is English only.

---

## 6. Regulated industry / third-party material

Tabular does not operate in a regulated industry. It is a general-purpose
developer tool, comparable to other database clients on the App Store.

- It provides no financial, medical, legal, gambling, or similar service.
- It contains no third-party copyrighted or licensed content.
- It connects only to servers the user already controls and supplies the
  address for. We provide no data, no data feed, and no content of our own.
- The app is open source under the AGPL; third-party components are open-source
  libraries used under their own licences.

---

## Regarding Guideline 3.2 (Other Business Models)

Tabular is not an internal or employee-only app. It is a general-purpose tool
for the public, distributed free on the App Store and developed in the open.
Public distribution is the correct channel for it.

---

## Short version for the Notes field

> Tabular is a database client and SQL editor for developers, DBAs, and data
> analysts. It connects to databases the user owns (MySQL, PostgreSQL, SQLite,
> SQL Server, MongoDB, Redis), browses schemas, runs SQL, and edits results.
>
> NO LOGIN IS REQUIRED. To review the app, launch it and tap "Sample Database"
> in the sidebar — a SQLite database bundled in the app opens with no setup,
> no file import, and no network. Expand it, tap a table, and run SQL such as
> `SELECT * FROM customers;`.
>
> An account is optional and only enables cross-device sync. Sign in with Apple
> is supported. Demo account if needed: <FILL IN email> / <FILL IN password>.
> There is only one account type.
>
> Account deletion: Settings → Sync & Account → Danger Zone → Delete Account.
> It deletes the account on our server immediately, inside the app.
>
> External services: our own api.tabular.id (optional sync only); Sign in with
> Apple / Google / GitHub (authentication only); the user's own database
> servers; and optionally OpenAI/Anthropic/Groq/GitHub Models for the AI
> assistant, which is inactive unless the user supplies their own API key.
>
> User-generated content is limited to Teams, which are invite-only by exact
> email/username/phone — there is no public feed or user discovery. Report and
> Block buttons are on every team member row; blocks are managed at Settings →
> Sync & Account → Blocked Users, and any member can leave a team.
>
> No in-app purchases, no ads, no analytics or tracking SDKs. Identical
> features in all regions. Not a regulated-industry app; no third-party
> protected content. Not an internal/employee app — it is a general-purpose
> developer tool for the public.

---

## Server setup required before Sign in with Apple works

Sign in with Apple is implemented end to end in the client and in
tabular-server, but it stays disabled until these exist in the Apple Developer
portal and in the server environment:

1. A **Services ID** (for example `id.tabular.database.signin`) with Sign in
   with Apple enabled, and `https://api.tabular.id/api/v1/auth/callback/apple`
   registered as a Return URL. Apple rejects `http`, including localhost.
2. A **Sign in with Apple key**; download the `AuthKey_XXXXXXXXXX.p8` once.
3. The **Team ID** and that key's **Key ID**.
4. `APPLE_CLIENT_ID`, `APPLE_TEAM_ID`, `APPLE_KEY_ID`,
   `APPLE_PRIVATE_KEY_PATH` and `APPLE_REDIRECT_URI` set on the server — see
   `env.example` in tabular-server.

Until step 4 is done the server answers the Apple sign-in attempt with
"Sign in with Apple is not configured on this server", so verify it on a device
before submitting.

---

## App Store screenshots (Guideline 2.3.3)

The previous submission's screenshots are the likely 2.3.3 exposure: they must
show the app actually in use, never the splash, icon art, or sign-in screen.
Capture these on a physical device at the required sizes (6.9" and 6.5" iPhone,
13" and 12.9" iPad):

1. Query editor with SQL typed in and a populated result grid below it — this
   is the one that has to be first.
2. Sidebar expanded showing the Sample Database's tables.
3. A result grid mid-edit, showing a cell being changed.
4. The schema/structure view of a table.
5. Optionally the AI assistant panel, only if the screenshot makes clear it
   needs the user's own API key.

Do not include: the launch screen, the app icon on its own, a marketing slide
with no UI, or the sign-in screen.

---

## Pre-submission checklist

- [ ] Sample Database opens on a physical device with no account
- [ ] Sign in with Apple completes on a physical device
- [ ] Delete Account completes and the app returns to signed-out state
- [ ] Report and Block buttons appear on team member rows, and Blocked Users
      lists the block afterwards
- [ ] A non-owner member can leave a team
- [ ] No "Check for Updates" item anywhere in the iOS UI
- [ ] Support email filled in above and reachable
- [ ] Someone is watching the server logs for `content report filed`
- [ ] Screenshots show the query editor and result grid in use, not the splash
      or sign-in screen (Guideline 2.3.3)
- [ ] `CURRENT_PROJECT_VERSION` in `Tabular.xcodeproj` bumped above the build
      number already uploaded — App Store Connect rejects a duplicate build
      number even when the marketing version is unchanged
      (`MARKETING_VERSION` is already 0.16.3, matching the crate)
- [ ] Demo credentials in App Store Connect are current
