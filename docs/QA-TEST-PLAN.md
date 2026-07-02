# QA Test Plan — RPGMaker Translator (Tauri v2 / Rust + React)

**Product**: Desktop app to translate RPGMaker MV/MZ games by hand or via AI.
**Stack**: Rust core (`src-tauri/src/`) behind `#[tauri::command]`, React 18 + TS + Vite frontend (`src/`), SQLite sidecar (`.rpgtl/project.db`), OS keychain for secrets.
**Repo**: `C:\Users\Mac\Works\translator-game`
**Reviewed for this plan**: `src-tauri/src/lib.rs`, `engine/{mod,mvmz,codes,protect}.rs`, `model.rs`, `project/{mod,db}.rs`, `ai/{mod,prompt,retry,openai,anthropic,gemini}.rs`, `keys.rs`, `src/{ipc,store,settings,langs}.ts`, `src/views/*.tsx`, `src/components/*.tsx`, `CLAUDE.md`, `tests/{extract_roundtrip,project_flow,tm_glossary}.rs`, fixture `src-tauri/tests/fixtures/mz-sample/`.

---

## 1. Scope

**In scope (V1)**: RPGMaker MV/MZ engine (`engine::mvmz::MvMzEngine`), project lifecycle (open/create/export), grid CRUD, TM, glossary + lint, AI translation (5 providers), key storage, desktop UI (React).

**Out of scope**: future engines (VX Ace, Ren'Py — `CLAUDE.md` "Adding an engine"), mobile targets (`cfg_attr(mobile, ...)` exists but unused), localization of the UI itself.

**Test levels**:
- Rust unit tests (`cargo test --lib`) — pure functions, no I/O.
- Rust integration tests (`cargo test --test <name>`) — full engine/project flow against `tests/fixtures/mz-sample`.
- Manual E2E — full desktop app, `pnpm tauri dev` build.
- (Recommended, currently absent) Frontend component tests and desktop E2E automation — see §9.

---

## 2. Test Environment

| Item | Detail |
|---|---|
| OS under test | Windows 11 (primary, dev machine) — **must also verify macOS + Linux before GA** because `keyring` uses per-OS backends (`apple-native`, `windows-native`, `sync-secret-service` — `Cargo.toml:29`) |
| Rust toolchain | edition 2021, `rust-version = "1.77"` (`Cargo.toml:7`) |
| Build commands | `cargo build`, `cargo test`, `pnpm build` (tsc + vite), `pnpm tauri build` (per `CLAUDE.md`) |
| Test fixture | `src-tauri/tests/fixtures/mz-sample/data/*.json` — synthetic MZ game; **read-only**, tests copy it to a temp dir first (`temp_game()` in `project_flow.rs` / `tm_glossary.rs`) |
| Local AI provider | Ollama (or LM Studio) at `http://localhost:11434/v1` for the "local" provider manual tests |
| Cloud AI providers | Need live test keys for OpenAI, Anthropic, Gemini, OpenRouter (or a mock HTTP server — see §9) |
| Frontend test infra | **None exists today** — no vitest/RTL, no Playwright/WebdriverIO config in `package.json`. Flagged in §9. |

**Exit criteria for a release build**: all `cargo test` green, all P0/P1 manual E2E scripts pass on Windows, no P0/P1 open bugs, smoke checklist (§8) clean on at least 2 browsers-equivalent (Chrome-based Tauri webview — only one renderer, so instead verify both light OS themes / DPI scales if app is theme-aware — otherwise N/A) and both a local and one cloud AI provider.

---

## 3. Coverage Analysis — existing automated tests vs. gaps

### 3.1 What's covered today

| Test file / test | Covers |
|---|---|
| `tests/extract_roundtrip.rs::detects_mvmz` | MZ detection (`data/` dir only), `describe()` file count |
| `::extract_finds_expected_units` | System.json (title, terms.messages, weaponTypes, null-skip in terms.commands), Actors (name/nickname/profile, note excluded by default), Items description, MapInfos name, Map001 displayName, dialogue (401) with control codes + grouping + speaker context from 101, choices (102) + When-choice (402), NPC map dialogue context, confirms 355 scripts NOT extracted by default |
| `::roundtrip_identity` | Full extract→inject(translation==source)→re-serialize semantic equality for **every file in the fixture** |
| `::inject_applies_only_target` | Injecting a single unit doesn't touch sibling nodes |
| `tests/project_flow.rs::open_edit_export_reopen` | `open_or_create` fresh-extract, edit persists, `export(backup=true)` patches + backs up, reopen reuses DB |
| `tests/tm_glossary.rs::tm_propagates_to_duplicate_sources` | TM upsert + `apply_tm` sibling-duplicate fill path (both TM-table match and sibling match fire together — not isolated) |
| `::glossary_crud_and_lint` | glossary CRUD, lint flags missing term, lint clears after fix |
| `engine/protect.rs` unit tests | mask/restore identity for `\C[2]`, `\V[7]`, `\.`, `\!`, `\N[1]`, `\FS[24]`, literal `\\`, `\{`/`\}`, plain text, empty string; sentinel reordering tolerance; dropped-sentinel detection |
| `ai/prompt.rs` unit tests | system/user message shape incl. glossary; plain array parse; fenced + out-of-order parse; missing-item error |

### 3.2 Gaps (nothing currently exercises these)

| Area | Gap | Why it matters |
|---|---|---|
| Engine — MV path | `mvmz::data_dir()` MV branch (`root/www/data/`) is **never exercised** — only MZ (`root/data/`) via the fixture | MV games are explicitly in scope (`CLAUDE.md`, task description) |
| Engine — opt-in extraction | `ExtractOpts{include_comments, include_plugin_args, include_scripts, include_notes}` — only the **default-off** state is asserted (scripts absent). None of the four toggles is ever flipped on | `codes.rs:53-56` gates 108/408/356/357/355/655 behind these; if the gate logic regresses, nothing fails |
| Engine — `protect::code_len` edge grammar | Unclosed `[` (`\C[2` no `]`), bracket-code with **no letter prefix** (`\[2]`), unclosed + no-letter (`\[2`) | `protect.rs:70-89` has 3 distinct branches here; only the "well-formed" and "no code" cases are covered by the existing 6 samples |
| Engine — message grouping | Blank/empty 401 line **inside** a message run | `mvmz.rs:433-444`: group state is set before the empty-string skip; untested whether the group survives across a blank line |
| Engine — choices with control codes | `102`/`402` array items containing `\C[n]` etc. | Pointer math (`ArrayAt`) is untested with embedded codes; codes there are never pre-masked at extraction time (masking only happens in `translate_units`), so this is purely a pointer/serialization risk |
| Engine — inject | **Stale pointer error path** — `val.pointer_mut(&u.pointer)` returning `None` (`mvmz.rs:94-102`) | Explicitly a hard-requirement error path (`anyhow!("stale pointer ...")`); zero coverage |
| Engine — detect | `detect_game` / `engine::detect` returning `None` for a **non-RPGMaker folder** | Only the positive detect path is tested |
| DB — `list_units` | `limit`/`offset` clamping (`db.rs:156-157`: `clamp(1,5000)`, `.max(0)`) | No boundary test at 0, negative, 5001+, exactly 5000 |
| DB — `list_units` search | `search` uses raw `LIKE '%q%'` — SQL wildcard chars `%`/`_` in user input untested | Could produce surprising matches |
| DB — `update_unit` | **No validation of the `status` string** — `db::update_unit` writes whatever string is passed straight into the `status` column (`db.rs:194-200`); only `Status::from_str` (used on *read*) defaults unknowns to `Untranslated` | See Bug Candidate BC-1 below — untested round-trip of a garbage status value |
| DB — `apply_tm` | The persisted-TM-table branch (`db.rs:288-295`, `n1`) is **not isolated** from the duplicate-sibling branch (`n2`) in `tm_propagates_to_duplicate_sources` — both fire in the same test | Task explicitly calls for "apply_tm from persisted TM table (not just duplicates)" |
| DB — glossary | No `UNIQUE` constraint on `glossary.term` (`db.rs:35-41`); no test for duplicate-term insert or for `case_sensitive=true` lint behavior | Duplicate terms would double-flag lint warnings |
| Export | `export(..., backup=false)` never tested | Explicitly requested; also untested: export with **zero applied units** (`touched.is_empty()` ⇒ no backup dir even if `make_backup=true`, `mvmz::inject` loop no-ops) |
| Export | Backed-up/touched source file **missing on disk** at export time (deleted externally) | `project::export` line 138 `if src.exists()` skips backup silently but `inject` will then hard-fail reading it — untested |
| AI — batch/split fallback | `ai::translate_batch_or_split` (`ai/mod.rs:107-131`) has **zero test coverage** — no mock `TranslationProvider` exists anywhere in the repo | This is the core AI resilience mechanism |
| AI — retry/backoff | `ai::retry::with_retry` / `status_is_retryable` (`retry.rs`) — zero tests | Pure logic, trivially unit-testable without network |
| AI — providers | `openai.rs`, `anthropic.rs`, `gemini.rs` — **zero tests** (request shape, header/auth, success parse, retryable vs. fatal status mapping, empty-content fatal path) | All 3 wire formats are hand-rolled JSON with no schema validation |
| AI — `list_models` | `ai::mod::list_models` (dedup/sort, per-provider URL shape, unknown-kind error) — zero tests | Powers the model picker; silent wrong-URL regressions wouldn't be caught |
| AI — `ProviderConfig` helpers | `temperature()`, `max_tokens()`, `batch_size()` clamp(1,200), `min_interval_ms()` from `rpm`, `needs_key()` — zero unit tests | Boundary math (rpm=0 vs 1 vs huge) |
| Command layer | `translate_units` (async, `lib.rs:286-451`) — the whole orchestrator (lock-drop-before-await, cancellation, chunking, partial-batch persistence, progress events) has **zero automated coverage**; same for `test_provider`, `list_models`, `detect_game`, `set_key/has_key/delete_key` | Highest-risk function in the codebase per `CLAUDE.md`'s own "invariants that span files" callout |
| Frontend | **No automated frontend tests exist at all** (no vitest/RTL/Playwright config in `package.json`) | `codesMismatch` regex (`UnitRow.tsx:6-12`), store logic (`store.ts`), settings persistence (`settings.ts`) are all hand-verified only |

---

## 4. New Unit / Integration Test Cases (Rust)

IDs prefixed by module. "Automatable" = Yes unless noted. Suggested file is where the test should live; new files should copy the `fixture()`/`temp_game()` helper pattern already used in `tests/project_flow.rs`.

### 4.1 Engine — MV/MZ (`engine::mvmz`, `engine::codes`, `engine::protect`)

| ID | Priority | Title | Suggested location | Steps / Assertion |
|---|---|---|---|---|
| RT-ENG-001 | P0 | MV data dir detected via `www/data/` | new `tests/fixtures/mv-sample/www/data/System.json` (+ minimal `Actors.json`) and a new `tests/mv_engine.rs` | Build a tiny MV-shaped fixture (System.json with `System.json` at `www/data/`, no `data/` dir). Assert `engine::detect()` returns the MV/MZ engine, `mvmz::data_dir()` resolves to `.../www/data`, `describe()` and `extract()` work identically to the MZ path. |
| RT-ENG-002 | P1 | Folder with **both** `data/` and `www/data/System.json` | same file | `data_dir()` prefers `data/` first (per code order in `mvmz.rs:118-126`) — assert MZ path wins. |
| RT-ENG-003 | P0 | Non-RPGMaker folder → `detect` returns `None` | `tests/mv_engine.rs` or `extract_roundtrip.rs` | `tempfile::tempdir()` with random unrelated files (or empty dir) → `engine::detect(&root).is_none()`. Also drive it through the command: call `lib.rs::detect_game` (needs `tauri::test`, see RT-CMD section) expecting `Ok(None)`. |
| RT-ENG-004 | P1 | `include_comments` opt-in extracts 108/408 | inline temp fixture (write a minimal `CommonEvents.json` with a 108 comment command) | `ExtractOpts{include_comments:true,..Default::default()}` → unit with `UnitKind::Comment` present; with default opts it's absent. |
| RT-ENG-005 | P1 | `include_plugin_args` opt-in extracts 356 (`At(0)`) and 357 (`At(3)`) | same pattern | Verify both codes' specific parameter index is pulled (357 at index 3, not 0 — easy to regress). |
| RT-ENG-006 | P1 | `include_scripts` opt-in extracts 355/655 | same pattern | With opt-in true, `UnitKind::Script` units appear (currently only the "absent by default" half is tested in `extract_finds_expected_units`). |
| RT-ENG-007 | P2 | `include_notes` opt-in on `note` fields for every `db_fields` file (Actors, Classes, Skills, Items, Weapons, Armors, Enemies, States) | table-driven test iterating `db_fields()` | Confirms the shared `note` gate in `extract_db_array` (`mvmz.rs:213`) for all 8 file kinds, not just Actors. |
| RT-ENG-008 | P1 | `protect::mask` — unclosed bracket with letter prefix | `engine/protect.rs` `#[cfg(test)]` module | `mask("\\C[2unclosed")` ⇒ `tokens == ["\\C"]`, text contains literal `[2unclosed` unmasked. |
| RT-ENG-009 | P2 | `protect::mask` — bracket code with **no** letter prefix, closed | same | `mask("\\[2]")` ⇒ treated as one token `"\\[2]"` (grammar allows letter-less bracket codes per `code_len` — verify this is intentional, not accidental). |
| RT-ENG-010 | P2 | `protect::mask` — bracket, no letters, unclosed | same | `mask("\\[2oops")` ⇒ `code_len` returns `None` ⇒ backslash copied as a literal char, no token created, no panic. |
| RT-ENG-011 | P0 | `protect::mask/restore` — string of **only** control codes (no natural-language text) | same | `mask("\\C[1]\\C[0]")` round-trips; document as the "invisible to translator" edge case referenced in TC-E2E-08. |
| RT-ENG-012 | P0 | Message grouping survives a **blank 401 line** mid-run | small in-test JSON via `serde_json::json!` fed through the private helper indirectly — i.e. write a temp `CommonEvents.json` with `[401 "Line1", 401 "", 401 "Line2"]` and call `extract()` | Exactly 2 units emitted (blank line produces none), and both share the same non-`None` `group` value. |
| RT-ENG-013 | P1 | Choices (`102`) / When-choice (`402`) containing embedded control codes | temp fixture | `parameters[0]` array item `"\\C[2]Yes\\C[0]"` extracts with `source` verbatim including the codes; pointer path `.../parameters/0/<i>` correct; round-trip identity still holds for the file. |
| RT-ENG-014 | P0 | `inject` — stale pointer error | `extract_roundtrip.rs` (or new `tests/inject_errors.rs`) | Extract units, mutate one `TransUnit.pointer` to a path that no longer exists (e.g. `/999/name`), mark it applied, call `inject()`. Assert `Err` containing `"stale pointer"` and that **no output file was partially written for that path** (or that the whole call errors before any file for that unit is written — confirm current all-or-nothing-per-file behavior by design). |
| RT-ENG-015 | P2 | `inject` — non-string param becomes a translation target inadvertently | temp fixture with a numeric field misidentified (defensive check) | Confirms `extract` never emits a unit for non-string JSON values (e.g. `note` numeric, malformed) — regression guard for `.and_then(\|v\| v.as_str())` gates throughout `mvmz.rs`. |
| RT-ENG-016 | P1 | Empty/malformed JSON data file | temp fixture where one file (e.g. `Skills.json`) is `""` or `"{not json"` | `extract()` returns `Err` (via `with_context("parsing {name}")`) rather than panicking; verify the error message names the offending file. |

### 4.2 Project / DB (`project::db`, `project::mod`)

| ID | Priority | Title | Steps / Assertion |
|---|---|---|---|
| RT-DB-001 | P1 | `list_units` limit boundary | `UnitFilter{limit: Some(0), ..}` ⇒ clamps to 1 row max; `Some(999999)` ⇒ clamps to 5000; `Some(-5)`... (type is `i64`, negative possible from a raw invoke) — assert `clamp(1,5000)` behavior at 0, 1, 5000, 5001. |
| RT-DB-002 | P2 | `list_units` offset negative | `offset: Some(-10)` ⇒ treated as 0 (`.max(0)`), doesn't error. |
| RT-DB-003 | P2 | `list_units` search with SQL wildcard chars | `search: Some("50%".into())` / `Some("_")` against fixture data (`"Restores 50 HP."`) — document actual behavior (wildcard chars are **not** escaped, so `%`/`_` in a real search behave as SQL wildcards, not literals) as either accepted-behavior or a candidate fix. |
| RT-DB-004 | P0 | `update_unit` with an unrecognized status string | Call `db::update_unit(conn, id, Some("x"), "NotAStatus")` directly, then `list_units`/`all_units` and confirm the round-tripped `TransUnit.status == Status::Untranslated` (current `Status::from_str` default) **while** `translation` is still `Some("x")` — this mismatched state is the crux of BC-1 below; assert it explicitly so a future fix changes this test intentionally. |
| RT-DB-005 | P0 | `apply_tm` — **TM-table-only** path, isolated | Fresh project, call `db::tm_upsert(conn, "Potion", "ยา")` directly (no sibling unit ever translated), then `apply_tm`. Assert every untranslated unit whose `source == "Potion"` becomes `Draft` with `translation == "ยา"`, and that units with other sources are untouched. This isolates `n1` from `n2` (`db.rs:288-317`). |
| RT-DB-006 | P1 | `apply_tm` does not overwrite non-`Untranslated` units | Pre-set a unit with source `"Yes"` to `status=Locked, translation="ห้ามแก้"`, ensure TM has a different value for `"Yes"`; `apply_tm` must leave it untouched (`WHERE status = 'Untranslated'` guard). |
| RT-DB-007 | P1 | Glossary duplicate term insert | `glossary_add("Potion", "ยา", ...)` twice ⇒ both rows persist (no UNIQUE constraint) ⇒ `glossary_lint` produces **two** warnings for the same unit/term — confirm this is the actual (possibly undesired) behavior. |
| RT-DB-008 | P1 | Glossary `case_sensitive=true` lint | Add glossary term `"HP"` case-sensitive; a unit whose source contains `"hp"` (lowercase) must **not** be flagged; a unit with exact-case `"HP"` and a translation missing the mapped wording **must** be flagged. |
| RT-DB-009 | P2 | Glossary CRUD with empty term/translation | Call `glossary_add("", "", None, false)` directly — current code has no validation; document whether this should be rejected (candidate for input validation, see BC-2). |
| RT-DB-010 | P1 | `export(project, backup=false)` | `temp_game()` fixture, translate one unit, `export(&proj, false)`. Assert `ExportResult.backup_dir.is_none()`, no `.rpgtl/backups/` dir created, game file still patched correctly. |
| RT-DB-011 | P1 | `export` with zero applied units | Fresh project (nothing translated), `export(&proj, true)`. Assert `files_written == 0`, `units_applied == 0`, `backup_dir.is_none()` (guarded by `!touched.is_empty()` in `project/mod.rs:129`), and the game's `data/` files are byte-identical to before (untouched). |
| RT-DB-012 | P1 | `export` when a touched source file was deleted externally | `temp_game()`, translate a unit in `System.json`, delete `data/System.json` from disk before calling `export`. Assert backup step **silently skips** that file (`if src.exists()`, `project/mod.rs:138`) but `inject()` then returns an `Err` reading the missing file — confirm the whole export fails cleanly (no partial `.rpgtl/backups/` left inconsistent, or document exactly what's left behind). |
| RT-DB-013 | P2 | Re-extract preserves existing edits | Open project, translate a unit, then call `open_or_create` again on the same root (simulating "re-import") — `insert_units` uses `INSERT OR IGNORE` keyed on `(file, pointer)` so the edit must survive. Not explicitly asserted today even though `project_flow.rs` reopens (it reopens without re-extracting since `unit_count() != 0`). Add a variant that also perturbs the game JSON with an extra new string and confirms the new unit is added **and** the old edit is preserved. |

### 4.3 AI layer (`ai::mod`, `ai::prompt`, `ai::retry`, `ai::openai`/`anthropic`/`gemini`)

Introduce a `MockProvider` (`struct MockProvider(Box<dyn Fn(&BatchReq) -> Result<Vec<String>> + Send + Sync>)` implementing `TranslationProvider`) local to a new `#[cfg(test)]` module in `ai/mod.rs`, or a shared test-only module — this unblocks RT-AI-001..004 without any network.

| ID | Priority | Title | Steps / Assertion |
|---|---|---|---|
| RT-AI-001 | P0 | `translate_batch_or_split` — batch succeeds | Mock returns `Ok(vec![...])` sized to `req.items.len()` ⇒ all `Some(...)`, in order, no fallback calls made. |
| RT-AI-002 | P0 | `translate_batch_or_split` — batch fails, falls back per-item, mixed outcome | Mock: batch call (items.len() > 1) returns `Err`; per-item calls succeed for even ids, fail for odd ids. Assert result vector aligns 1:1 with input order, `Some` for even, `None` for odd, and that the mock recorded exactly `1 + items.len()` calls (one batch attempt + N singles). |
| RT-AI-003 | P1 | `translate_batch_or_split` — single-item request that fails | `req.items.len() == 1` and batch `Err` ⇒ short-circuits to `vec![None]` **without** a redundant single-item retry (per `ai/mod.rs:115`). |
| RT-AI-004 | P2 | `translate_batch_or_split` — empty items | `req.items` empty ⇒ returns `vec![]`, provider never called. |
| RT-AI-005 | P0 | `retry::with_retry` — retryable then success | Counting closure: fails `CallError::Retryable` twice, succeeds 3rd try, `max_tries=4` ⇒ `Ok`, called exactly 3 times. |
| RT-AI-006 | P0 | `retry::with_retry` — exhausts retries | Always `Retryable` with `max_tries=3` ⇒ `Err` after exactly 3 attempts. |
| RT-AI-007 | P0 | `retry::with_retry` — fatal short-circuits | First attempt `Fatal` ⇒ `Err` immediately, exactly 1 call, no sleep/backoff. |
| RT-AI-008 | P1 | `retry::status_is_retryable` table | `429`→true, `500..599`→true (boundary 499→false, 600→false), `400/401/403/404`→false. |
| RT-AI-009 | P1 | `ProviderConfig` boundary helpers | Table: `batch_size` None→40, `Some(0)`→1 (clamp floor), `Some(500)`→200 (clamp ceiling), `Some(75)`→75. `min_interval_ms`: `rpm=None`→0, `Some(0)`→0, `Some(1)`→60000, `Some(120)`→500, `Some(61)`→983 (integer division, not rounded — verify exact value). `needs_key`: `"local"`→false, `"openai"/"anthropic"/"gemini"/"openrouter"`→true, unknown kind→true. |
| RT-AI-010 | P1 | `ai::mod::list_models` response parsing (needs a tiny mock HTTP server — see §9 for `wiremock` recommendation) | Per-kind: OpenAI/OpenRouter/Local shape `{"data":[{"id":"m1"},{"id":"m2"}]}` → sorted+deduped `["m1","m2"]`; Anthropic same shape via `/v1/models`; Gemini shape `{"models":[{"name":"models/gemini-pro"}]}` → `["gemini-pro"]` (prefix stripped). Non-2xx response ⇒ `Err` including status+body. Unknown `cfg.kind` ⇒ `Err("unknown provider kind: ...")` without any HTTP call. |
| RT-AI-011 | P0 | `openai::OpenAiCompat` — success, retry-then-success, fatal 4xx, malformed JSON body | Mock HTTP server: assert request URL is `{base}/chat/completions`, `Authorization: Bearer <key>` header present when key given and absent for local/no-key, OpenRouter-only headers (`HTTP-Referer`, `X-Title`) present only when `is_openrouter`. 429 then 200 ⇒ succeeds after retry. 401 ⇒ `Fatal`, no retry. Non-JSON 200 body ⇒ `Fatal` (parse error), not silently swallowed. |
| RT-AI-012 | P0 | `anthropic::Anthropic` — success/error mapping + missing key | Assert `x-api-key`/`anthropic-version` headers, request body shape (`system`, `messages[0].content`). Empty `content` array in a 200 response ⇒ `Fatal("empty response")`. No key provided ⇒ immediate `Err("Anthropic requires an API key")` **before** any HTTP call. |
| RT-AI-013 | P0 | `gemini::Gemini` — success/error mapping + missing key | Assert key passed as `?key=` query param, URL embeds `req.model`. Empty `candidates[0].content.parts` ⇒ `Fatal`. Missing key ⇒ immediate `Err` pre-HTTP. |
| RT-AI-014 | P1 | `prompt::parse_batch_response` — no JSON array anywhere (pure prose) | `"Sorry, I can't do that."` ⇒ `Err("no JSON array found...")`. Distinguish from the already-tested "array present but incomplete" case. |
| RT-AI-015 | P2 | `prompt::parse_batch_response` — object-wrapped array variants | `{"items":[...]}` and `{"data":[...]}` both parse via `array_from_value`'s object fallback — only bare-array and fenced-array are tested today. |
| RT-AI-016 | P2 | `prompt::build_messages` — `source_lang = "auto"` (any case) vs. explicit language | Assert the "(auto-detect it, commonly Japanese or English)" phrasing only appears when `source_lang` is empty or case-insensitively `"auto"`; explicit `"Japanese"` produces `"Japanese text"`. |

### 4.4 Command layer (`lib.rs`) — via `tauri::test`

Tauri v2 ships a mock runtime (`tauri::test::{mock_builder, mock_context, noop_assets}` / `MockRuntime`) suitable for invoking `#[tauri::command]` functions with a real `AppState` and asserting on emitted events. This requires adding `tauri = { version = "2", features = ["test"] }` (or the dedicated test crate, per current Tauri version) to `[dev-dependencies]` — currently absent from `Cargo.toml`.

| ID | Priority | Title | Steps / Assertion |
|---|---|---|---|
| RT-CMD-001 | P0 | `translate_units` — lock not held across `.await` (no deadlock under concurrency) | Build a mock app + `AppState`, open a project, kick off `translate_units` with a `MockProvider` that sleeps 200ms per call; concurrently call `get_stats`/`list_units` from another task while the translation is in flight. Assert the concurrent call returns promptly (isn't blocked for the whole translation duration) — this is the direct regression test for the `CLAUDE.md` invariant. |
| RT-CMD-002 | P0 | `translate_units` — cancellation stops **between** batches, not mid-call | 3 batches queued, cancel flag set after batch 1 completes (simulate via a hook in the mock provider) ⇒ `summary.cancelled == true`, `summary.translated` reflects only completed batches, DB has partial writes persisted (already-written batches are not rolled back). |
| RT-CMD-003 | P1 | `translate_units` — empty work returns immediately | Scope selects only already-translated units with `overwrite:false` ⇒ `work` empty ⇒ `summary.requested == 0`, no provider call, no progress event emitted. |
| RT-CMD-004 | P0 | `translate_units` — masked-restore failure marks unit failed, does not write | Mock provider strips a sentinel from its response for one item; assert that unit's status/translation are **unchanged** in DB (still whatever it was before) and `summary.failed` incremented — direct test of the "not written on mangled restore" invariant. |
| RT-CMD-005 | P1 | `translate_units` — `overwrite:false` skips already-translated, `overwrite:true` includes them | Table-driven over the `overwrite` flag. |
| RT-CMD-006 | P2 | `translate_units` — no API key stored for a key-requiring provider | `config.kind = "openai"`, no key set ⇒ command returns `Err("no API key stored for provider 'openai'")` before any work is gathered. |
| RT-CMD-007 | P1 | `detect_game` command — happy path and `None` path | Wraps RT-ENG-001/003 through the actual command signature (`Result<Option<DetectResult>, String>`), confirming the camelCase serde contract (`DetectResult` `#[serde(rename_all="camelCase")]`) matches `src/ipc.ts`'s `DetectResult` interface field names. |
| RT-CMD-008 | P1 | `set_key`/`has_key`/`delete_key` round trip | Set a key for a throwaway provider name (e.g. `"test-provider"` to avoid clobbering a real stored credential), `has_key` true, `delete_key`, `has_key` false, `delete_key` again is idempotent (no error) — mirrors `keys.rs`'s `Err(keyring::Error::NoEntry) => Ok(())` handling. **Run only in an environment with a usable OS keychain (CI runners often lack one — see §9).** |

---

## 5. Manual E2E Test Scripts

Preconditions common to all: app built via `pnpm tauri dev` (or an installed build), a scratch copy of a small real or synthetic RPGMaker MV/MZ game folder (never point manual tests at a git-tracked or otherwise irreplaceable game folder — Export mutates it in place).

### TC-E2E-01 — Import & detect (happy path)
**Priority**: P0
1. Launch app → `ImportView` shown (no project open).
2. Click "Choose game folder…" → OS folder picker → select a valid MZ game root (has `data/System.json`).
3. **Expected**: button shows "Detecting…" then a detect card appears with correct `engineName` ("RPGMaker MV/MZ"), `fileCount`, `dataDir`.
4. Change "From"/"To" language selects (e.g. Japanese → Thai).
5. Click "Open project".
6. **Expected**: button shows "Extracting…"; on completion, main grid view loads; top bar shows correct engine name, root path, and stats chips (`total` == sum of all extracted units, `todo` == `total` on first import).

### TC-E2E-02 — Import: non-RPGMaker folder
**Priority**: P0
1. Choose an arbitrary folder with no `data/System.json` or `www/data/System.json`.
2. **Expected**: error text "No supported game engine detected in this folder." shown; no detect card; "Open project" not offered.

### TC-E2E-03 — Import: MV (deployed, `www/data/`) game
**Priority**: P0
1. Point at an MV game folder that has `www/data/System.json` and no top-level `data/`.
2. **Expected**: identical happy path to TC-E2E-01; `dataDir` in the detect card ends in `www\data` (or `www/data`).

### TC-E2E-04 — Manual translate → export → verify game file
**Priority**: P0
1. From an opened project, filter to a specific file (e.g. `System.json`) via the file dropdown.
2. Edit the "gameTitle" row's translation textarea, type a translation, click elsewhere (blur) to commit.
3. **Expected**: row's left border color changes (Untranslated→Draft color per `UnitRow.tsx` `statusColor`), top-bar `draft` chip increments, `todo` chip decrements.
4. Change the row's Status dropdown to "Translated".
5. Click "Export → game" in the top bar.
6. **Expected**: success text "Exported 1 units → 1 files (backup saved)"; open the game's actual `data/System.json` on disk in a text editor — the `gameTitle` value matches what was typed, file stays syntactically valid compact JSON, and `.rpgtl/backups/<timestamp>/System.json` exists with the **original** title.
7. Re-open the file in the app's own re-import (close project, reopen same folder) — the export applied translation should still show for that unit (DB persisted, not overwritten by re-extract since `INSERT OR IGNORE` on `(file,pointer)`).

### TC-E2E-05 — AI translate happy path — Local (Ollama)
**Priority**: P0
Preconditions: Ollama running locally with a small model pulled (e.g. `llama3.1`).
1. Open Settings (⚙), select "Local (Ollama / LM Studio)" tab.
2. Base URL defaults to `http://localhost:11434/v1`; set Model to the pulled model name (or use "↻ Refresh" to populate the datalist from `list_models`).
3. Click "Test connection" → **Expected**: "✓ <some Thai/translated text>" for the fixed "Hello, world!" probe.
4. Close settings, filter grid to a small file, set "AI translate" mode to "Shown (current filter)", click "Run".
5. **Expected**: progress bar animates, `done/total` counter updates live (via `translate://progress` events), on completion "Done — N translated" summary shown, grid rows refresh with new translations and `Translated` status/color.
6. Verify a row containing a control code (e.g. `Welcome, \C[2]hero\C[0]!`) — its translated text still contains `\C[2]` and `\C[0]` verbatim (control-code masking survived the AI round trip). No "⚠ control codes differ" badge on that row.

### TC-E2E-06 — AI translate happy path — one cloud provider (e.g. OpenAI or Anthropic)
**Priority**: P0
1. In Settings, switch to the cloud provider tab, enter Model, paste a valid API key, click "Save".
2. **Expected**: key input clears, placeholder becomes "•••••••• (stored)"; a "Clear" button appears.
3. "Test connection" → "✓ ...". "↻ Refresh" models → dropdown populates from `list_models`.
4. Run a translate on a small selection as in TC-E2E-05.
5. **Expected**: same as TC-E2E-05. Additionally verify the app never displays the raw API key anywhere (devtools localStorage should contain only non-secret `ProviderConfig`, never the key — inspect `localStorage['rpgtl.settings.v1']`).
6. Restart the app entirely. **Expected**: "Test connection" still works without re-entering the key (keychain-persisted) but the key field is still empty/masked (never read back into the input).

### TC-E2E-07 — Cancel mid-run
**Priority**: P0
1. Start an AI translate run over a large-enough selection that it takes several batches (set Batch size small, e.g. 2, in Settings → Shared, to force multiple batches on a modest unit count).
2. While running, click "Cancel".
3. **Expected**: run stops promptly (within one in-flight batch's completion — not instantly mid-HTTP-call, per the `state.cancel` check between chunks in `translate_units`); summary shows "Cancelled — N translated"; units from completed batches are persisted as Translated; units not yet reached remain Untranslated (not marked failed).
4. Click "Run" again (without Cancel) on the same "Shown" scope with overwrite off. **Expected**: only the still-untranslated remainder is processed.

### TC-E2E-08 — Placeholder / control-code mismatch warning badge
**Priority**: P1
1. Manually edit a translation for a row whose source contains a control code (e.g. remove `\C[2]` from the translated text, or duplicate it).
2. **Expected**: on blur, "⚠ control codes differ" badge appears next to the textarea (`UnitRow.tsx` `codesMismatch`); textarea gets a `warn` CSS class.
3. Restore the exact code sequence (reordering is OK, e.g. move `\C[0]` earlier) → badge disappears (front-end check is a sorted-multiset compare, order-insensitive by design, matching the backend's `protect::restore` tolerance).
4. Test a unit whose source is **entirely** control codes (RT-ENG-011 case, if such a unit exists/can be crafted in the test game) — confirm the UI doesn't crash and the warning logic behaves sensibly with an "empty" visible source.

### TC-E2E-09 — Language switching (JA→TH, EN→TH, Auto)
**Priority**: P1
1. On Import, set From=Japanese, To=Thai, open project. After opening, verify TranslateBar's language selects reflect Japanese→Thai.
2. Change TranslateBar's "From" select to "Auto" mid-project (via `setLanguages`). **Expected**: persists (`set_languages` command → `meta` table `source_lang`); reopening the project later still shows "Auto".
3. Run an AI translate with source=Auto against known-English source text; verify system prompt effectively auto-detects (indirect check: translation is sensible Thai, not a literal echo).
4. Repeat with From=English, To=Thai and confirm distinctly different prompt behavior isn't required to test directly, but functional translation quality is acceptable for a smoke check.
5. Target language list never offers "Auto" (`TARGET_LANGS` has no Auto) — confirm the dropdown genuinely has no such option.

### TC-E2E-10 — Model refresh (Ollama)
**Priority**: P2
1. Settings → Local tab → "↻ Refresh" with Ollama running and ≥1 model pulled. **Expected**: datalist populates, hint text "N model(s) — pick from the list or type." shown.
2. Stop the Ollama service, click "↻ Refresh" again. **Expected**: `modelsErr` shown (network error text), no crash, previous model list optionally stale but UI recovers on next successful refresh.
3. With Ollama running but zero models pulled: **Expected**: "No models returned." message (`list.length === 0` branch in `SettingsView.tsx`).

### TC-E2E-11 — Glossary + lint
**Priority**: P1
1. Open Glossary panel, add a term/translation pair (e.g. "Potion" → "ยาฟื้นฟู"), optional note, toggle case-sensitive off.
2. **Expected**: appears in the table immediately; inline edit (blur to save) works for term/translation/note; delete (🗑) removes it.
3. Manually mistranslate a unit containing "Potion" so the translation lacks "ยาฟื้นฟู".
4. Open Lint panel. **Expected**: warning listed with correct file/term/expected; clicking the file link filters the grid to that file and closes the modal.
5. Fix the translation to include the glossary wording, reopen Lint. **Expected**: "✓ No glossary inconsistencies found."
6. Run an AI translate over units containing the glossary term — verify the resulting translations consistently use the glossary wording (glossary is fed into the system prompt).

### TC-E2E-12 — TM apply
**Priority**: P1
1. Translate and mark "Translated" one unit whose source string appears more than once in the game (duplicate text, e.g. a common "Yes"/"No" choice).
2. Click "Apply TM" in the top bar.
3. **Expected**: success text "Filled N from memory"; the sibling duplicate unit(s) become `Draft` with the same translation text; stats refresh.

### TC-E2E-13 — Reopen persistence
**Priority**: P0
1. Fully close the app (not just "Close" the project) after making several edits (some Draft, some Translated, some Locked) without exporting.
2. Relaunch, reopen the same game folder.
3. **Expected**: `freshlyExtracted` is false; all statuses/translations exactly as left; stats chips match.

### TC-E2E-14 — Backup restore
**Priority**: P1
1. Perform TC-E2E-04's export. Note the backup path shown/inferable (`.rpgtl/backups/<unix-ts>/`).
2. Manually copy the backed-up file back over the patched game file (simulating a user "undo").
3. Reopen the game in the app (or just inspect on disk) — confirm the original untranslated text is restored, and the app's own DB (`.rpgtl/project.db`) is unaffected (translations still shown in-app even though the game file was rolled back — the DB is the source of truth until the next export).
4. Export again — confirm it works normally against the restored file (no stale-pointer errors, since structure is unchanged).

### TC-E2E-15 — Close/Reopen keychain key lifecycle
**Priority**: P2
1. Set a key for provider A, don't set one for provider B.
2. Restart app. Settings → provider A shows "stored" placeholder; provider B shows empty "paste key…" placeholder.
3. Clear provider A's key. Restart app again — confirm it's genuinely gone (Test connection should now fail with the "no API key stored" error).

---

## 6. Edge / Boundary Cases

### 6.1 Scale
| ID | Case | Priority | Expected / Risk |
|---|---|---|---|
| TC-EDGE-01 | Huge game: 10,000+ translatable units | P1 | Import completes in reasonable time; grid stays smooth via `@tanstack/react-virtual` (`GridView.tsx`, `estimateSize: 96`, `overscan: 12`); scrolling doesn't jank. **Known limitation to verify, not "fix": `translate_units`'s "all untranslated" scope hard-caps candidates at `limit = Some(5000)` (`lib.rs:316`) — running "All untranslated" on a 10k-unit game will only ever pick up the first 5000 by id per run; user must run it twice.** Confirm this is documented/communicated in the UI (currently it is **not** — no message tells the user only 5000 were considered). |
| TC-EDGE-02 | `list_units` limit/offset extremes | P2 | See RT-DB-001/002 — clamp behavior at the API boundary. |
| TC-EDGE-03 | Very long single string (e.g. a 5000-character note/description) | P2 | Grid row renders without layout breakage; AI batch `max_tokens` (default 4096) may truncate a long translation — verify graceful handling (not silent data loss) when a single item's expected output exceeds `max_tokens`. |

### 6.2 Malformed / unexpected input data
| ID | Case | Priority | Expected |
|---|---|---|---|
| TC-EDGE-04 | Empty JSON file (`Skills.json` is `""`) | P0 | `extract()` errors per-file with context (`with_context("parsing {name}")`); confirm the **whole import doesn't silently produce a half-populated DB** — since `extract()` returns a single `Result` for the whole engine call, one bad file should fail the entire `open_or_create`, not partially succeed. Verify this end-to-end (not just unit-level). |
| TC-EDGE-05 | Malformed JSON (`{not valid`) | P0 | Same as above. |
| TC-EDGE-06 | Non-string param where a string is expected (e.g. `parameters[0]` is a number for a 401 command) | P1 | Extraction skips it silently (`.and_then(\|v\| v.as_str())` returns `None`) — confirm no panic, and document that such content is **not** flagged to the user as "possibly untranslated" (silent skip is a UX gap, not a crash risk). |
| TC-EDGE-07 | Missing `System.json` mid-session (deleted after project opened, before Export) | P1 | See RT-DB-012 for the automated angle; manually confirm the error surfaced to the user is legible, not a raw Rust panic string. |
| TC-EDGE-08 | Data dir with 0 `.json` files but `System.json` absent too | P2 | `data_dir()` returns `None` → same as "non-RPGMaker folder". |

### 6.3 AI provider failure modes
| ID | Case | Priority | Expected |
|---|---|---|---|
| TC-EDGE-09 | Missing API key for a key-requiring provider | P0 | `translate_units`/`test_provider` return the exact `"no API key stored for provider '<kind>'"` error; UI surfaces it in the `err` span, doesn't crash the run. |
| TC-EDGE-10 | Provider returns wrong item count (fewer/more than requested) | P0 | `parse_batch_response` errors ⇒ `translate_batch_or_split` falls back to per-item; net effect in UI: those items still translate (slower) rather than all failing. Verify this end-to-end with a manual mock (e.g. a tiny local proxy that intentionally truncates the response) or by starving a real local model's context so it drops items. |
| TC-EDGE-11 | Provider returns fenced ` ```json ... ``` ` | P0 (already unit-tested at parse level; needs one live E2E confirmation) | Real models (esp. GPT/Claude) commonly wrap JSON in fences — confirm at least one live cloud provider actually does this and the app still succeeds. |
| TC-EDGE-12 | Provider returns prose / refuses ("I can't translate that content") | P1 | Falls through the same fallback path as TC-EDGE-10 → those items end up `failed`; UI shows "N failed" without crashing; failed units remain `Untranslated`, visibly re-runnable. |
| TC-EDGE-13 | HTTP 429 (rate limited) then success | P1 | `with_retry` backs off and succeeds — verify via a provider/proxy that can simulate 429, or by setting an aggressive `rpm` combined with a provider that actually rate-limits. |
| TC-EDGE-14 | HTTP 5xx sustained (all retries exhausted) | P1 | Run ends with those items `failed`, not an app crash; error surfaces per-batch, not fatal to the whole run (since `translate_units` swallows per-item failures into `summary.failed`). |
| TC-EDGE-15 | Rate-limit throttle (`rpm` setting) actually spaces out requests | P2 | Set `rpm=6` (10s/request), run a multi-batch job, time the batches — confirm ≥`min_interval_ms` between batch starts (`min_interval_ms = 60000/rpm`). |
| TC-EDGE-16 | Network fully offline mid-run | P1 | Requests fail as retryable (reqwest connection error) → eventually exhausts retries → items marked failed; app remains responsive, Cancel still works. |
| TC-EDGE-17 | Unicode / Thai text round-trip through control-code masking | P0 | Source contains Thai/CJK text mixed with control codes (`"สวัสดี \C[2]คุณ\C[0]!"` as an artificial source, or a Thai-source game if available) — confirm `protect::mask` byte-indexing (`input.as_bytes()` + `chars().next()`) never panics or corrupts multi-byte UTF-8 sequences adjacent to a `\` byte. This is a real risk area since `mask()` walks bytes but falls back to char-boundary copying only in the "not a code" branch — confirm a `\` immediately followed by a multi-byte character (e.g. `"\ก"`, not a valid code) doesn't mis-slice. |
| TC-EDGE-18 | Concurrent edits — two "users" (two app instances or rapid double-submit) editing the same unit | P2 | Single-project-at-a-time design (`AppState.project: Mutex<Option<Project>>`) — the real risk is **two windows of the same app pointed at the same `.rpgtl/project.db`** (SQLite `WAL` mode allows concurrent readers/one writer) — verify behavior isn't silent data loss (last-write-wins is acceptable if documented; a crash or corrupted DB is not). |
| TC-EDGE-19 | Export while a translate run is in-flight | P2 | Both acquire the same `Mutex`; confirm no deadlock, and that Export sees a consistent snapshot (either pre- or post- the in-flight batch, never a torn read — SQLite transaction boundaries should guarantee this at the row level, but the interaction of `translate_units`'s per-batch re-lock with an overlapping `export_project` call is untested). |

---

## 7. Bug Candidates Found During Code Review

These are **not confirmed failures** (no test run performed — Read-only review) but are concrete, reproducible-by-inspection risks worth a triage pass before/with the corresponding test cases above.

### BC-1 — Invalid `status` string silently corrupts stats aggregation
**Severity**: Medium **Priority**: P2
**Where**: `project/db.rs::update_unit` (writes the raw `status: &str` with no validation) vs. `Status::from_str` (`model.rs:32-40`, defaults any unrecognized string to `Untranslated` on read).
**Repro (code-level)**:
1. Call `db::update_unit(conn, id, Some("x"), "Approved")` directly (bypasses the frontend's closed `<select>` of `STATUSES`, but reachable via any direct `invoke("update_unit", { status: "Approved" })` from devtools, a future bulk-edit feature, or a scripting API).
2. Call `db::stats(conn)`.
**Expected**: total should equal the sum of the 5 named buckets.
**Actual**: `stats()`'s `GROUP BY status` match arm has a `_ => {}` catch-all (`db.rs:229`) — `s.total` is incremented unconditionally but no per-status bucket is, so `total > untranslated+draft+translated+reviewed+locked`. The frontend's progress bar (`App.tsx` `pct` calc using `stats.total - stats.untranslated`) would then over-report completion.
**Suggested fix direction**: validate `status` against `Status::from_str`/reject unknowns at the command boundary (`lib.rs::update_unit`), or make `db::update_unit` take a typed `Status` instead of `&str`.
Covered by: RT-DB-004.

### BC-2 — No validation on glossary term/translation emptiness or duplicates
**Severity**: Low **Priority**: P3
**Where**: `project/db.rs::glossary_add`/`glossary_update`, no `UNIQUE` on `glossary.term`.
**Repro**: call `glossary_add(conn, "", "", None, false)` — succeeds, inserts an empty-term row. Call it twice with identical `(term, translation)` — two rows persist. `glossary_lint` then double-counts warnings for duplicate terms.
**Impact**: mostly cosmetic (frontend already guards with `.trim()` checks in `GlossaryView.tsx`), but the command layer itself has no defense-in-depth.
Covered by: RT-DB-007, RT-DB-009.

### BC-3 — "All untranslated" translate run silently caps at 5000 units, no user-facing indication
**Severity**: Medium **Priority**: P2
**Where**: `lib.rs::translate_units`, `f.limit = Some(5000)` (line 316) when no explicit `ids` are given.
**Repro**: on a game with >5000 untranslated units, select "All untranslated" mode and Run. Only 5000 are processed; `TranslateSummary.requested` reports 5000, which is technically correct but nothing tells the user more work remains beyond "todo" chip still being > 0.
**Impact**: confusing UX on very large games; not data loss, but could be mistaken for a bug ("I ran translate-all and it's still not done").
Covered by: TC-EDGE-01.

### BC-4 — `list_units` `search` does not escape SQL `LIKE` wildcards
**Severity**: Low **Priority**: P3
**Where**: `project/db.rs::list_units`, `format!("%{q}%")` built from raw user input with no escaping of `%`/`_`.
**Impact**: a user searching for a literal `%` or `_` gets unexpected extra matches. Not a security issue (parameterized otherwise, no injection), just a correctness/UX nit.
Covered by: RT-DB-003.

---

## 8. Regression / Pre-release Smoke Checklist

Run this on every release candidate build, in order, on a scratch game folder:

- [ ] `cargo test` all green, warning-free build (`CLAUDE.md`: "Rust must build warning-free")
- [ ] `pnpm build` (tsc strict + vite) clean
- [ ] Import a valid MZ game → detect → open → grid populates (TC-E2E-01)
- [ ] Import a valid MV (`www/data/`) game → same (TC-E2E-03)
- [ ] Import a garbage folder → clean "not detected" error, no crash (TC-E2E-02)
- [ ] Manual edit → status change → Export → verify on-disk file changed, backup created (TC-E2E-04)
- [ ] Export with a fresh project (nothing translated) doesn't error, doesn't create an empty backup dir
- [ ] AI translate — Local provider — small batch, verify control codes intact (TC-E2E-05)
- [ ] AI translate — one cloud provider — small batch (TC-E2E-06)
- [ ] Cancel mid-run behaves correctly, no partial corruption (TC-E2E-07)
- [ ] Glossary add/edit/delete + Lint flags and clears correctly (TC-E2E-11)
- [ ] Apply TM fills duplicates (TC-E2E-12)
- [ ] Close app fully, relaunch, reopen project — all edits persisted (TC-E2E-13)
- [ ] Set + clear an API key, confirm it never appears in `localStorage` (devtools check)
- [ ] Grid virtualization smooth on a large file (1000+ rows in one file)
- [ ] No Rust panics in the terminal/log for any of the above (panics currently propagate as ugly `Err` strings at best — grep console output)

---

## 9. Automation Strategy

### 9.1 Pyramid for this codebase

```
Manual E2E (desktop, ~15 scripts in §5):     covers cross-cutting UI/AI/keychain flows that are
                                              expensive to automate today (no E2E harness exists)
                                                    ▲
Rust integration tests (cargo test --test *):covers engine/project/DB flows against the fixture
  — extend: MV path, opt-in toggles, export   (~30% of effort)
  variants, stale pointer, tauri::test command
  layer for translate_units
                                                    ▲
Rust unit tests (cargo test --lib):          protect.rs, prompt.rs, retry.rs, ProviderConfig,
  — extend: retry, provider HTTP mocks,       provider wire formats via wiremock
  ProviderConfig boundaries                   (~50% of effort — cheapest, highest ROI)
                                                    ▲
Frontend tests:                              currently ZERO — see 9.3
```

### 9.2 Recommended additions, in priority order

1. **P0 — `ai::retry` + `ai::mod::translate_batch_or_split` unit tests with a `MockProvider`** (RT-AI-001..009). Zero external dependencies, pure logic, highest bug-catching ROI, directly protects the "one bad item can't sink a batch" resilience guarantee.
2. **P0 — HTTP-mocked provider tests** (RT-AI-010..013) using the `wiremock` crate (add to `[dev-dependencies]`). Locks down the 3 hand-rolled wire formats against silent breakage (e.g. an Anthropic API version bump).
3. **P0 — `tauri::test`-based command tests for `translate_units`** (RT-CMD-001..006). Requires adding `features = ["test"]` to the dev-dependency `tauri` entry (or Tauri's dedicated mock-runtime crate for v2 — confirm exact crate/feature name against the installed Tauri version before implementing). This is the single highest-value addition given it's explicitly called out as the riskiest function in `CLAUDE.md`.
4. **P1 — Engine gap-fillers** (RT-ENG-001..016): new small ad-hoc fixtures built with `tempfile` inside each test (don't touch the shared `mz-sample` fixture per the existing test-authoring convention — `CLAUDE.md`: "Integration tests copy the fixture to a temp dir before writing, so they never dirty it").
5. **P1 — DB/export gap-fillers** (RT-DB-001..013).
6. **P2 — Frontend unit/component tests.** Add **Vitest** + **@testing-library/react** (currently zero frontend test infra in `package.json`). Priority targets: `codesMismatch` regex boundary cases in `UnitRow.tsx` (mirror the Rust `protect::code_len` edge cases from RT-ENG-008..010 in TS, since the two implementations can silently diverge), `store.ts`'s `editUnit`/`setStatus` optimistic-update logic (mock `api.*`), `settings.ts` persistence round-trip (`localStorage` mock).
7. **P2 — Desktop E2E automation.** Tauri apps are **not** plain web pages — Playwright cannot drive the native window directly. The standard approach is **WebdriverIO + `tauri-driver`** (WebDriver protocol against the OS webview), not Playwright, unless the team is willing to test the React app in isolation under plain Vite/Playwright with `window.__TAURI__`/`invoke` mocked (fast, but doesn't exercise the real Rust backend or native dialogs). Recommend: keep true cross-process E2E manual (§5) for now; if investing in automation, start with the mocked-IPC Playwright approach for pure-UI regression (grid rendering, filter bar, modal flows) and reserve `tauri-driver` for a handful of true smoke flows (import → translate → export) closer to GA.

### 9.3 CI considerations
- **Keychain-dependent tests** (RT-CMD-008, `keys.rs`) need a real OS credential backend; headless Linux CI runners typically lack a Secret Service provider — either skip these in CI (`#[ignore]` + run manually) or provision `gnome-keyring`/equivalent in the CI image.
- **Cloud AI provider tests** should never hit live APIs in CI (cost, flakiness, secret management) — use `wiremock` for all CI-run provider tests; reserve real-key tests for the manual E2E pass (§5, TC-E2E-06) before each release.

---

## 10. Exit Criteria (Definition of Done for a release)

- All `cargo test` pass, zero warnings.
- All P0 test cases in §4 and §5 pass (once implemented/executed).
- No open P0/P1 bugs; BC-1..BC-4 triaged (fixed or explicitly accepted with a note in release notes).
- Regression checklist (§8) fully green on the target OS(es) for the release.
- At least one full manual pass of TC-E2E-05 (local) and TC-E2E-06 (cloud) on the actual release build (not just `tauri dev`).

---

## SharePoint Tracking Template

| Test ID | Module | Title | Priority | Status | Tester | Defect ID |
|---|---|---|---|---|---|---|
| RT-ENG-001 | Engine | MV data dir detected via www/data/ | P0 | | | |
| RT-CMD-001 | Command | translate_units lock-not-held-across-await | P0 | | | |
| TC-E2E-01 | Import | Import & detect happy path | P0 | | | |
| ... | | | | | | |
