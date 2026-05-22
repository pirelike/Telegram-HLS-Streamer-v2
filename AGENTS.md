
# AGENTS.md

> This file is auto-loaded by OpenCode for both the `planner` and
> `investigator` agents. Keep it concise — it goes into every context window.
> For verbose domain rules, use files in `.opencode/rules/*.md` (already
> referenced via the `instructions` field in `opencode.json`).

## Project overview

[Replace with 2–4 sentences describing what this project does, its
language/runtime, and the deployment target.]

## Build, test, and lint commands

- **Install:** `[command]`
- **Build:** `[command]`
- **Test (canonical):** `[command]`
- **Test (single file):** `[command]`
- **Lint:** `[command]`
- **Type-check:** `[command]`
- **Format:** `[command]`

These are the canonical verification commands. The investigator should cite
this section when reporting verification commands to the planner.

## Conventions

- **Style:** [e.g., "We use 2-space indentation; semicolons required."]
- **Imports:** [e.g., "Absolute imports via `@/` alias; no deep relative paths."]
- **File naming:** [e.g., "kebab-case for files, PascalCase for components."]
- **Test naming:** [e.g., "Co-located `*.test.ts` files; no top-level `__tests__/`."]
- **Error handling:** [e.g., "Throw typed errors from `lib/errors.ts`; never bare `throw new Error(...)`."]

## Architecture invariants

- [Anything that should never change without explicit discussion.
  Examples: "All public API calls go through `src/api/client.ts`."
  "Database access only via the repository layer in `src/db/`."]

## Do-not-touch zones

- [Paths where edits require special caution. Examples:
  `src/migrations/` — never modify existing migration files.
  `config/secrets/` — read-only; values are populated at deploy time.]

## Agent guidance

- The default agent is `planner`. Tab to `build` for implementation.
- For non-trivial work (3+ files, API/schema changes, >30 min effort, or
  cross-cutting concerns), use `planner` first to produce a reviewable plan.
- For trivial work (single-file edits, typo fixes, adding a log line),
  switch directly to `build`.
- The planner delegates codebase exploration to the `investigator` subagent
  — do not bypass this by reading dozens of files yourself.
- Never invoke the investigator for questions about user intent — that is
  the planner's job via the `question` tool.

## ⚠️ ALAPELV — OLVASD EL MINDEN FELADAT ELŐTT

**A legegyszerűbb működő megoldást írd meg. Mindig.**

Az AI agentek krónikusan túlkomplikálnak mindent — absztrakciós rétegeket, felesleges interface-eket, “flexibilitásra” tervezett rendszereket gyártanak olyan problémákra, amik 20 sor szkripttel megoldódnának. Ez a projekt **kifejezetten tiltja** ezt a viselkedést.

### Konkrét szabályok

1. **YAGNI** — ne írj kódot olyan use case-re, ami nincs ma. Ha egyszer kell, majd akkor hozzáadjuk.
1. **Egy függvény, egy dolog**. Ha a megoldás egy 30 soros függvény egy fájlban, akkor az a megoldás. Ne csinálj belőle osztályt, factory-t, plugin rendszert.
1. **Ne vezess be új dependency-t**, ha a standard library / a már használt csomag megoldja. `difflib` elég — ne húzz be `deepdiff`-et. `httpx` elég — ne húzz be `aiohttp`-t.
1. **Ne tervezz “jövőbeli kiterjeszthetőségre”**. Nincs `AbstractScraperFactory`, nincs `BaseParserStrategy`. Egy konkrét Scraper osztály van. Ha másodikra is szükség lesz, *akkor* refaktorálunk — a második konkrét eset alapján, nem képzelgésből.
1. **Ne írj wrappert wrapperre**. Ha a `httpx.get()` megteszi, ne csinálj `HttpClientService.fetch_url()`-t körülötte.
1. **Ne vezess be config opciót “hátha”**. Hardcode-old az értéket. Ha tényleg kell majd konfigolni, 10 mp átírni.
1. **Ne tegyél try/except-et mindenre**. A hiba legtöbbször legyen hangos — dobja fel, hadd lássuk. Csak ott kapj el kivételt, ahol tényleg tudsz vele mit kezdeni (pl. scraper retry).
1. **Ne írj absztrakciót egy implementációhoz**. Két, lehetőleg három konkrét használat nélkül ne vezess be interface-t/protokollt.
1. **Kevesebb fájl jobb mint több**. Ha egy modul 200 sor, maradjon egy fájl. Ne szedd szét 5 fájlra `types.py`, `exceptions.py`, `utils.py`, `service.py`, `repository.py` néven.
1. **Először írd meg működőre, utána szépítsd** — de csak ha kell. Sokszor a “csúnya” 40 soros változat jobb, mint a “tiszta” 120 soros.

### Ha túlkomplikált útra indulsz, kérdezd meg magadtól

- *Működne ez egy darab függvényként?* Általában igen.
- *Használom ezt az absztrakciót 2+ helyen most, vagy csak elképzelem, hogy majd fogom?* Ha az utóbbi → töröld.
- *Van-e ennek a dependency-nek olyan feature-je, amit tényleg használok?* Ha nincs → a stdlib jó lesz.
- *A tesztem bonyolultabb, mint a tesztelt kód?* Rossz jel — vagy a kód túl absztrakt, vagy nem éri meg tesztelni.

### Ha mégis komplexebb megoldást írsz

**Indokold** a PR-ben / a kód komment tetején, hogy *miért* nem elég az egyszerűbb verzió. Konkrét okot adj, ne elméletit (“skálázhatóság”, “clean code” nem ok). Jó indok: “azért kell async, mert 10k jogszabályt párhuzamosan scrape-elünk és szinkron 6 órát tart”.

## Projekt-specifikus szabályok

- Ez egy Rust/Axum/SQLite/Telegram/FFmpeg alapú HLS streamer. A meglévő modulokhoz igazodj; ne vezess be új keretrendszert vagy dependency-t dokumentált, konkrét ok nélkül.
- A runtime adat nem forráskód: `streamer.db`, `streamer.db-*`, `uploads/`, `processing/`, `target/` és `TEST_FILE.mkv` nem dokumentációs vagy refaktorálási alap.
- Dokumentációs szerepek:
  - `README.md`: gyakorlati belépési pont fejlesztőknek és üzemeltetéshez.
  - `REBUILD.md`: részletes viselkedési/specifikációs referencia.
  - `ROADMAP.md`: megvalósítási állapot és elfogadási lista.
  - `src/**/guide.md`: source-folder module térképek.
- `guide.md` használata új session/agent indulásakor:
  - Mielőtt egy `src/` alatti modulban dolgozol, olvasd el a legközelebbi releváns `guide.md`-t: először `src/guide.md`, majd az érintett folder saját guide-ját (pl. `src/api/guide.md`, `src/api/jobs/guide.md`).
  - A guide a navigációs térkép: mit tartalmaz a folder, melyik fájl miért felel, milyen irányba mennek a függőségek, és hova szabad/tilos új kódot tenni.
  - A guide nem helyettesíti a kód olvasását. Használd belépési pontnak, aztán ellenőrizd a konkrét fájlokat, mielőtt módosítasz.
  - Ha fájlt mozgatsz, modult szétbontasz, vagy felelősségi határt változtatsz, frissítsd ugyanabban a változtatásban az érintett `guide.md`-t is.
- Kódmódosítás után futtass célzott ellenőrzést. Általános változtatásnál `cargo test`; formázásnál `cargo fmt --check`; lintnél `cargo clippy --all-targets --all-features`.
- Médiafolyamot érintő változtatásnál, ha Telegram credential be van állítva, a kézi end-to-end ellenőrzés: `python3 scripts/upload_and_wait.py TEST_FILE.mkv --timeout 7200 --start-timeout 180 --request-timeout 180`.
- Ha dokumentációt frissítesz, tartsd szinkronban az agent fájlokat: `AGENTS.md`, `CODEX.md`, `CLAUDE.md`.
- **Telegram feltöltési limit**: A Telegram Bot API maximális fájlmérete jelenleg 20 MB (`TELEGRAM_MAX_FILE_SIZE=20971520`). Minden feltöltött fájlnak (`.m4s`, `.ts`, `.vtt`, `.jpg`) ez alatt kell lennie. A `telegram_max_file_size` config értéket ne emeld a Bot API aktuális limitje fölé — de ha a Telegram megemeli a limitet, frissítsd ennek megfelelően.
- **Szegmens méret limit**: A `SEGMENT_TARGET_SIZE` (alapértelmezett: `15728640`) szintén felhasználó által konfigurálható. A feltöltési ceiling változása esetén igazítsd hozzá.
- **Minőségmegőrzés**: SOHA ne növeld a kódolási sebességet a videó minőségének rovására. Tilos `-preset ultrafast`-ot vagy más speed-over-quality FFmpeg beállítást használni teljesítményoptimalizálás céljából.
- **Túlméretes szegmens kezelése**: Ha egy szegmens meghaladja a `telegram_max_file_size` által meghatározott limitet, számold ki a legmagasabb bitrate-et ami még belefér, és azzal kódold újra. A felbontást NE csökkentsd — ugyanaz a felbontás, maximális bitrate. Ha a minimális értelmes bitrate (32k) mellett is túl nagy, keyframe határoknál vágd szét (`-c copy`, nincs újrakódolás).
- **SEASON END — automatikus Pull Request**: Ha a felhasználó "SEASON END"-et ír, automatikusan commit-old és push-old az összes módosított követett fájlt, majd nyiss Pull Request-et a GitHub-on a `.env` fájlban lévő `GITHUB_TOKEN`, `GITHUB_REPO_OWNER`, `GITHUB_REPO_NAME` és `GITHUB_BASE_BRANCH` változók alapján (`gh auth login --with-token`, `git add`, `git commit`, `git push`, `gh pr create`). A munkamenet végén mindig ellenőrizd a `git diff --name-only` kimenetét: ha vannak módosított követett fájlok és a felhasználó elfelejtette beírni a "SEASON END"-et, akkor is automatikusan commit-old, push-old és nyiss PR-t — ugyanezzel a módszerrel. Soha ne veszíts el munkát amiatt mert a felhasználó elfelejtett parancsot kiadni.

# Project Instructions

## Default workflow

Use the OpenCode orchestrator workflow.

The user normally talks only to `orchestrator`.

The orchestrator should:
1. Understand the task.
2. Delegate cheap codebase exploration to `glm-explorer`.
3. Use `glm-architect` for architecture, migrations, complex refactors, and design-heavy tasks.
4. Use `codex-implementer` as the default implementation worker.
5. Use `glm-implementer-strong` when the user explicitly asks for GLM implementation, when Codex is unavailable, or when GLM-5.1 is useful as a second implementation perspective.
6. Use `gpt-reviewer` before finalizing non-trivial code changes.
7. Synthesize the result for the user instead of dumping raw subagent logs.

## Agent routing policy

### `orchestrator`

Main coordinator.

Responsibilities:
- understand user intent;
- inspect the repo only as needed;
- choose the correct worker agent;
- prevent scope creep;
- ask for approval before risky operations;
- synthesize results;
- ensure implementation work gets reviewed.

The orchestrator should avoid direct implementation unless the task is trivial.

### `glm-explorer`

Cheap read-only repository exploration.

Use for:
- finding relevant files;
- tracing call paths;
- summarizing modules;
- locating tests;
- identifying conventions;
- mapping dependencies.

Must not edit files.

### `glm-architect`

High-capability GLM-5.1 planning worker.

Use for:
- architecture decisions;
- refactor plans;
- migration plans;
- risky backend changes;
- long-context codebase reasoning;
- dependency and compatibility analysis.

Must not edit files.

### `codex-implementer`

Primary implementation worker.

Model target:
- GPT-5.3-Codex
- medium reasoning

Use for:
- normal feature implementation;
- bug fixes;
- test creation;
- frontend changes;
- backend changes;
- scoped refactors;
- API integration work.

Rules:
- keep diffs minimal;
- follow existing project style;
- do not broaden scope;
- add or update tests when behavior changes;
- never commit or push.

### `glm-implementer-strong`

Strong GLM-5.1 implementation worker.

Use for:
- GLM-specific implementation;
- second implementation perspective;
- long-context implementation;
- fallback if Codex is unavailable;
- difficult scoped work where GLM-5.1 may be useful.

Rules:
- keep diffs minimal;
- follow existing project style;
- do not broaden scope;
- add or update tests when behavior changes;
- never commit or push.

### `gpt-reviewer`

Final review worker.

Use before finalizing non-trivial code changes.

Review for:
- correctness;
- regressions;
- security;
- maintainability;
- edge cases;
- missing tests;
- API compatibility;
- project convention violations.

Must not edit files.

## Implementation rules

- Prefer small, reviewable diffs.
- Do not perform broad refactors unless explicitly requested.
- Do not change public APIs unless required.
- Do not remove tests unless explicitly justified.
- Add or update tests for behavior changes.
- Run targeted validation where practical.
- Never commit or push unless the user explicitly asks.
- Never modify secrets, `.env` files, private keys, or credential files.
- Never run destructive shell commands without explicit user approval.

## Validation policy

Prefer targeted validation first.

Examples:
- affected unit tests;
- typecheck;
- lint;
- build;
- focused integration tests.

Avoid running very expensive full-suite commands unless necessary or requested.

## Git policy

Allowed without special approval:
- `git status`
- `git diff`
- `git log`
- `git branch`

Requires explicit user approval:
- `git add`

Forbidden unless explicitly requested:
- `git commit`
- `git push`
- force pushes
- tag creation
- release publication

## Response format after code changes

When work is complete, summarize:

1. What changed
2. Files changed
3. Validation run
4. Reviewer verdict
5. Remaining risks or follow-up

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

When the user types `/graphify`, invoke the `skill` tool with `skill: "graphify"` before doing anything else.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- Dirty graphify-out/ files are expected after hooks or incremental updates; dirty graph files are not a reason to skip graphify. Only skip graphify if the task is about stale or incorrect graph output, or the user explicitly says not to use it.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
