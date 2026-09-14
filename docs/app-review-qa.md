# Device QA and screen recording — before resubmitting to App Review

Everything here needs a physical iPad and iPhone on the current iOS. None of it
can be verified from a build machine, and Apple explicitly asked for a recording
made on real hardware.

Work top to bottom. Anything that fails is a resubmission blocker, not a
"note it and ship" item — this app has already been rejected once under 2.1.

---

## 0. Build the thing that will actually ship

```bash
./apple/scripts/publish_xcode.sh ios archive
```

That archives `Tabular.xcodeproj`, scheme `Tabular-iOS`. The separate
`ios/Xcode/TabulariOS/` project is **not** what ships — do not test that one.

Before archiving, bump `CURRENT_PROJECT_VERSION` in `Tabular.xcodeproj` past
whatever build number was last uploaded. App Store Connect refuses a repeat
build number even when the marketing version is unchanged.

Distribute the archive through TestFlight and install that on the devices,
rather than running a debug build from Xcode: a debug build behaves differently
around sandboxing and signing, and Apple reviews the TestFlight artefact.

---

## 1. First launch, no account (Guideline 2.1)

This is the sequence that most likely caused the last rejection: on iOS every
file dialog returns nothing, so before the sample database existed there was
literally nothing a reviewer could do.

| # | Step | Expected |
|---|---|---|
| 1.1 | Delete any previous install, then launch fresh | App reaches the main window without a crash |
| 1.2 | Look at the sidebar | A connection named **Sample Database** is present |
| 1.3 | Expand it | `customers`, `products`, `orders` appear |
| 1.4 | Tap `customers` | 8 rows, readable in the grid |
| 1.5 | Run `SELECT * FROM products;` | 7 rows |
| 1.6 | Run `SELECT c.name, p.name, o.quantity FROM orders o JOIN customers c ON c.id=o.customer_id JOIN products p ON p.id=o.product_id;` | 10 rows, no error |
| 1.7 | Edit a cell and save | Change persists after collapsing and reopening the table |
| 1.8 | Force-quit and relaunch | Sample Database still there, and **not duplicated** |

If 1.2 fails, stop. Check the device log for `Could not create the sample
database` — seeding is best-effort and only logs on failure.

## 2. No out-of-store update surface (Guideline 2.5.2)

| # | Step | Expected |
|---|---|---|
| 2.1 | Open the settings menu | **No** "Check for Updates" item |
| 2.2 | Open Preferences and read every tab | **No** "Update" tab |
| 2.3 | Leave the app open a few minutes from cold start | No update dialog appears |

## 3. Sign in with Apple (Guidelines 4.8 and 2.1)

Server config must be in place first — see the Sign in with Apple section of
`app-review-notes.md`. Until it is, this fails with "Sign in with Apple is not
configured on this server".

| # | Step | Expected |
|---|---|---|
| 3.1 | Settings → Sync & Account → Sign In | **Sign in with Apple** is the first, most prominent button |
| 3.2 | Tap it | Safari opens Apple's sign-in page — this step was completely dead before the `open_url` fix, so confirm it really opens |
| 3.3 | Complete sign-in, return to the app | App picks up the session within a few seconds |
| 3.4 | Check the account screen | Email shown; display name populated on first sign-in |
| 3.5 | Look for a manual token field | **No** "Enter token manually" section on iOS |

Return to the app promptly at 3.3: the token is collected by polling, and the
poll gives up after 180 seconds of wall-clock time — including time spent with
the app suspended in the background.

Verify Google and GitHub sign-in too; the shared `open_url` path changed for all
three providers.

## 4. Report, block, leave (Guideline 1.2)

Needs two accounts. Call them A (yours) and B (a second device).

| # | Step | Expected |
|---|---|---|
| 4.1 | As A, create a team and add B by email | B sees the team after refreshing |
| 4.2 | As B, expand the team's Members | ⚑ Report and 🚫 Block buttons on A's row |
| 4.3 | As B, tap Report | Form with five reasons and a details field |
| 4.4 | Submit it | Toast confirms; server log shows `content report filed` |
| 4.5 | As B, tap Block on A, confirm | Toast confirms; the team disappears from B's list |
| 4.6 | As A, refresh teams | B is gone from the member list |
| 4.7 | As A, try to add B again by email | Fails as "user not found" — deliberately indistinguishable from a real miss |
| 4.8 | As A, search for B in add-member | B does not appear |
| 4.9 | As B, Settings → Sync & Account → Blocked Users | A is listed |
| 4.10 | Tap Unblock | Row disappears; A can add B again afterwards |
| 4.11 | As B, in a team B does not own, tap **Leave** | B is removed and the team disappears from B's list |

4.5 and 4.11 are the two that were genuinely broken before this round — a member
previously had no way out of a team they were added to without consent. 4.5 also
exercises the multi-table `DELETE ... JOIN` in `block_user`, which has never
been run against a real MySQL instance.

## 5. Account deletion (Guideline 5.1.1(v))

Use a throwaway account; this is irreversible.

| # | Step | Expected |
|---|---|---|
| 5.1 | Settings → Sync & Account → scroll down | **Danger Zone** with Delete Account |
| 5.2 | Tap Delete Account | Modal lists what is erased |
| 5.3 | Type a wrong email | Delete button stays disabled |
| 5.4 | Type the exact account email | Button enables |
| 5.5 | Confirm | Toast names the deleted account; app returns to signed-out |
| 5.6 | Verify server-side | The `users` row is gone, and so are its connections, queries, history, vault keys |
| 5.7 | Keep using the app | Sample Database and querying still work signed out |

If the account owned a team, confirm the team and its members are gone too —
that cascade is intentional and is what the modal warns about.

## 6. General stability (Guideline 2.1)

- Run on iPhone as well as iPad; check nothing is clipped at phone width.
- Rotate through every supported orientation on both.
- Background the app for several minutes mid-query, then return.
- Watch for crashes across the whole pass. Apple reviews on physical devices,
  and a single crash is its own 2.1 rejection.

---

## Screen recording

One take, on a physical iPad, starting from the home screen. Apple asked for the
typical user flow beginning at launch, plus account creation, deletion, and the
UGC report/block mechanisms.

Shot list, in order:

1. Tap the app icon — show the launch.
2. Tap **Sample Database**, expand it, tap `customers`.
3. Type and run a query; let the result grid fill.
4. Edit one cell and save.
5. Open Settings → Sync & Account. Pause on the line saying an account is
   optional.
6. **Sign in with Apple**, all the way through, back into the app.
7. Open Teams → a team → Members. Show ⚑ and 🚫 on a member row. Open the
   report form so the reasons are readable, then cancel.
8. Settings → Sync & Account → Blocked Users — show the list.
9. Danger Zone → Delete Account → type the email → confirm.
10. Land back on the signed-out app and run one more query, showing it still
    works without an account.

Keep it unhurried enough to read each screen. Do not cut steps 7 and 9 — those
are the two Apple named specifically.

---

## Resubmission

1. Fill in every `<FILL IN>` in `app-review-notes.md`: demo credentials and the
   support email.
2. Paste the short version into App Review Information → Notes.
3. Attach or link the recording.
4. Replace the screenshots per the 2.3.3 section — the first must be the query
   editor in use, never the splash or sign-in screen.
5. Reply to the rejection with the long version, answering all six points in
   the order Apple asked them.
6. Submit.
