# cordis (TS) upstream fact base — core parity

> Reference index for every core-v2 parity assertion. Every factual claim
> cites the upstream source as `path:line` relative to the clone root
> `/tmp/opencode/cordis` at pinned rev
> **`8cc9e33fab69e2d0476d126baaf2acb24e6a6ab4`**
> (2026-08-13 21:48:18 +0800, "chore: update readme (#45)"; zero drift
> since v1's port — re-verified 2026-08-31).
> Verify: `cd /tmp/opencode/cordis && git rev-parse HEAD` (must print the
> hash above; `git status --porcelain` empty). Re-clone:
> `git clone https://github.com/cordiverse/cordis`.
> Scope: **core only** — facts about `/tmp/opencode/deepseek-harness` belong
> to the harness effort, not here. Format model: v1's
> `docs/harness/upstream-facts.md` (pinned-rev + `path:line`), content
> re-derived from source.

> **Current Cordis terminology:** this historical fact base predates ADR 0039. Where it says `Fork`, read that as the former Cordis spelling for the current `FiberHandle`; upstream itself still has no `Fork` symbol. Historical API names such as `Fork::update` are left unchanged as evidence.

How to use this file:

- Every entry carries a `Parity:` mark. Ticket 07 (upstream
  parity inventory) ratified these into the three-way ledger:
  **carried-verbatim / port-freedom / deliberately-omitted** (plus
  deliberately-diverged, tracked separately from plain omissions) —
  see ADR 0006. The marks below stand as ratified except where ADR 0006
  says otherwise (§1.3 emit is fail-fast, not v1's log sketch).
- v1's lesson (fifth-pass #9/#13, sixth-pass #13): an upstream-parity claim
  without a `path:line` against the pinned rev is worthless. Nothing in this
  file is cited from memory; every line range below was re-read at the pinned
  rev. Where v1's `DESIGN.zh-CN.md` disagrees with upstream, the discrepancy
  is recorded in §1 — prominently, not buried.

Source map at the pinned rev (core unless noted): `packages/core/src/` —
`context.ts` (78), `events.ts` (178), `fiber.ts` (486), `index.ts` (7),
`logger.ts` (246), `reflect.ts` (281), `registry.ts` (214), `service.ts` (80),
`utils.ts` (278). Adjacent in-scope packages: `packages/timer/src/index.ts`
(142), `packages/loader/src/` (+`config/`), `packages/utils/src/index.ts` (42,
`List<T>`). Upstream behavioral spec: `packages/core/tests/*.spec.ts`.

## Contents

- [1. Discrepancies: where v1's claims meet upstream](#1-discrepancies)
- [2. Events and dispatch](#2-events-and-dispatch)
- [3. Fiber lifecycle, effects, disposal](#3-fiber-lifecycle-effects-disposal)
- [4. Settle protocol: epoch and inertia](#4-settle-protocol-epoch-and-inertia)
- [5. Services, visibility, notification](#5-services-visibility-notification)
- [6. Context, scope, isolation](#6-context-scope-isolation)
- [7. Registry](#7-registry)
- [8. Logger](#8-logger)
- [9. Timer](#9-timer)
- [10. Loader](#10-loader)
- [11. internal/* event inventory](#11-internal-event-inventory)
- [12. Port-freedom index (no upstream counterpart)](#12-port-freedom-index)
- [13. Deliberate omissions and divergences ledger](#13-omissions-and-divergences)
- [14. Could not verify](#14-could-not-verify)

## 1. Discrepancies

Where v1's `DESIGN.zh-CN.md` (or findings §2) asserts something about
upstream that the pinned rev contradicts. Each entry: what v1 claimed, what
upstream actually does, and the citation.

1. **"Upstream registry entries are only removed on explicit `delete()`
   (empty shells persist)" — FALSE at this rev.**
   v1: `DESIGN.zh-CN.md:844-846` ("`RegistryService._internal` 条目仅在显式
   `delete()` 时移除、空壳常驻"), used to frame v1's `prune_if_idle` as
   "compensation for our own deviation". Upstream actually prunes: when a
   fiber's spawn-effect teardown runs and it was the last fiber of its
   runtime, it removes the runtime entry —
   `packages/core/src/fiber.ts:182-187`
   (`remove(); if (!runtime.fibers.length) {
   this.ctx.registry.delete(runtime.callback) }`). Explicit `delete()` is the
   *other* path (`packages/core/src/registry.ts:162-171`). Consequence for
   v2: v1's `prune_if_idle` is upstream parity for the empty-runtime case,
   not merely self-compensation; what v1 truly deviates on is per-spawn
   `Anonymous(n)` runtimes (upstream shares one runtime per callback,
   `packages/core/src/registry.ts:199-205`).
2. **"Failed apply leaves epoch at the failure fingerprint" is a v1
   divergence, NOT upstream parity.**
   v1: `DESIGN.zh-CN.md:257-267` is honest that it "刻意不移植 epoch 重置",
   but any v2 parity table must not cite it as carried. Upstream's `_reload`
   catch sets `this._runner.epoch = INACTIVE`
   (`packages/core/src/fiber.ts:421-426`), which routes the cycle tail into
   `_unload` for failure cleanup (`fiber.ts:427-434`). v1 keeps the
   fingerprint so that same-fingerprint notifications are absorbed; upstream
   has no `set_service`-style notification to absorb
   (notify fires only from provide/unprovide and ACTIVE transitions —
   `packages/core/src/reflect.ts:175-203,205-227`,
   `fiber.ts:362-368`), so the retry storm v1 guards against is unreachable
   upstream. Parity status: deliberately-diverged (documented in v1).
3. **Upstream `emit` propagates a throwing listener synchronously to the
   dispatcher — it does not log-and-continue.**
   `packages/core/src/events.ts:96-99` has no containment; upstream's own
   spec pins it: `expect(() => root.emit(event)).to.throw('test')` —
   `packages/core/tests/events.spec.ts:87-90`. v1's scheme-B sketch assigned
   `Flow::Failed` in emit to "记日志" (`DESIGN.zh-CN.md:127-135`). v2 must
   decide this consciously: upstream = fail-fast propagation.
4. **Stale line citations in v1 (content verified, lines drifted).**
   - `DESIGN.zh-CN.md:995` cites `events.ts:148-155` for serial; serial is
     `packages/core/src/events.ts:101-107` at this rev (bail:
     `events.ts:109-115`).
   - `DESIGN.zh-CN.md:720` cites `logger.ts:169–199` for the ring-buffer
     exporter; the facts sit at `packages/core/src/logger.ts:171`
     (`bufferSize = 1000`) and `logger.ts:189-201` (built-in buffer
     exporter).
5. **Root-fiber uid semantics differ by representation.**
   Upstream: root fiber owns `uid = 0` and is permanently ACTIVE
   (`packages/core/src/fiber.ts:200-212`, uid at `:201`, ACTIVE at `:203`,
   epoch `''` at `:206`); "disposed" is `uid === null` (`fiber.ts:104`,
   set at `:180`, checked at `:349`). v1: uid `0` is the destroyed sentinel
   and real fibers start at 1. The *invariants* (root never dies; disposed
   fibers fail registration; first real fiber uid is 1 —
   `packages/core/src/registry.ts:126,136-138` uses `++_counter` from 0)
   are parity; the encoding of "dead" (null vs 0) is port freedom.
6. **Upstream shared isolate realms are garbage-collected; v1's intern table
   is not.**
   Upstream deletes a `GlobalRealm` label when no entry references it anymore
   (`packages/loader/src/config/isolate.ts:151-168`, in the
   `loader/partial-dispose` handler). v1's `Root.shared_isolates` only grows
   (`DESIGN.zh-CN.md:815-819` records the boundedness argument + revisit
   trigger). Parity status: carried-with-rework candidate for ticket 07.

## 2. Events and dispatch

- **Five dispatch modes are the upstream vocabulary**: `'emit' | 'parallel' |
  'serial' | 'bail' | 'waterfall'` — `packages/core/src/events.ts:14`
  (`DispatchMode`). Registration surface is just `on`/`once` with
  `options` (`events.ts:29-30`); overload-level thisArg variants at
  `events.ts:19-28`. Parity: carried-verbatim (vocabulary);
  v1's marker-trait *typing* of listener shapes is port freedom (§12).
- **Bail truthiness**: `isBailed(v) = v !== null && v !== false && v !==
  undefined` — `packages/core/src/events.ts:6-8`. So upstream reads a `false`
  return as "no answer". v1/v2 `Option` (`Some(false)` is an answer) is a
  documented deliberate divergence (`DESIGN.zh-CN.md:1021-1023`).
- **serial**: sequential `await` per listener, first bailed result returns —
  `packages/core/src/events.ts:101-107`. **bail**: identical loop minus
  `await` — `events.ts:109-115`. The two differ only by awaiting (confirmed
  by inspection: `events.ts:104` vs `events.ts:112`), which licenses v1's
  "same implementation, two intent-names" convergence. Parity:
  carried-verbatim.
- **Listener errors propagate and abort the remaining chain** (fail-closed):
  no try/catch in serial/bail (`packages/core/src/events.ts:101-115`);
  upstream spec: serial rejects (`packages/core/tests/events.spec.ts:106-109`),
  bail throws synchronously (`events.spec.ts:125-128`). This is the
  grounding for v1's 2026-08-26 P1 fail-closed fix
  (`DESIGN.zh-CN.md:989-1023`). Parity: carried-verbatim.
- **parallel**: `Promise.allSettled`, rejections aggregated into one
  `AggregateError` thrown after all settle — `packages/core/src/events.ts:
  89-94`; "a rejecting listener must not short-circuit the others" is pinned
  by `packages/core/tests/events.spec.ts:57-71`. Parity: carried-verbatim
  (v1's `AggregateError` channel).
- **emit**: synchronous loop, no error containment —
  `packages/core/src/events.ts:96-99`; throw propagates
  (`packages/core/tests/events.spec.ts:87-90`). See discrepancy §1.3.
- **waterfall is an onion**: the tail callback is popped from args, each
  layer receives `(...args, next)`; `next()` shifts the next callback or, at
  exhaustion, runs the tail — `packages/core/src/events.ts:117-126`. First
  registered runs first (outermost); `prepend` unshifts (more outer) —
  `events.ts:128-134` (`options.prepend ? 'unshift' : 'push'`). Spec:
  composition `(value, next) => value + next()` doubles the value through
  two layers, tail `() => 2` — `packages/core/tests/events.spec.ts:131-140`.
  Parity: carried-verbatim (v1's return to the onion,
  `DESIGN.zh-CN.md:649-682`, is correct).
- **A layer that ignores `next` short-circuits everything downstream,
  including the tail** — upstream's implicit-veto footgun:
  `packages/core/tests/events.spec.ts:144-152` (cb3 returns without calling
  next; cb4 and the tail never run; result is cb3's value). v1 makes
  short-circuit an around-only power and forces mapping layers to delegate
  (`DESIGN.zh-CN.md:665-668`) — port freedom tightening, anchored here.
- **`next` takes no parameters; transforms travel by mutating the shared
  args array** — `packages/core/src/events.ts:119-124` (`args.pop()` tail,
  closure over `args`, `args.push(next)`). v1's owned `next.call(args)` is
  a deliberate representation divergence (`DESIGN.zh-CN.md:671-672`).
- **"Call next at most once" is runtime discipline, unenforced** — double
  calling `next` double-shifts and can re-run the tail
  (`packages/core/src/events.ts:120-125`). v1's consuming `FnOnce`
  `Next<E>` makes it a type error (`DESIGN.zh-CN.md:668-672`): port freedom
  anchored on this line.
- **Single hooks table per event**: `_hooks: Record<keyof any, Hook[]>` —
  `packages/core/src/events.ts:45-46`; hooks carry `{ ctx, callback,
  prepend?, global? }` (`events.ts:35-43`). No second table for around-style
  listeners — the `internal/update` per-fiber routing is the only split (see
  below). Parity: carried-verbatim (v1's single-table + `HookKind` mixing).
- **Per-fiber hook routing exists upstream, built on `internal/listener`**:
  non-global `internal/update` listeners are diverted into
  `fiber._hooks['internal/update']` at registration —
  `packages/core/src/events.ts:54-60`; the service then composes them into
  one global waterfall where each captured layer calls
  `next` and finally delegates to the caller's tail —
  `events.ts:62-69`. v1 replaced this with `waterfall_scoped` +
  per-fiber scope nodes (`DESIGN.zh-CN.md:700-706`): carried-with-rework
  (same semantics — per-fiber update pipeline with external veto —
  different mechanism).
- **Registration is a fiber effect with a stable label**: hooks are inserted
  inside `this.ctx.fiber.effect(...)`, label `` `ctx.on(${name})` `` —
  `packages/core/src/events.ts:128-134,155-157`; unregister finds by callback
  identity (`events.ts:136-142`). Parity: carried-verbatim
  (`DESIGN.zh-CN.md:925-961`).
- **Registration gate**: `on` calls `assertActive()` before anything —
  `packages/core/src/events.ts:150`; `plugin` likewise —
  `packages/core/src/registry.ts:197`. `assertActive` throws
  `CordisError('INACTIVE_EFFECT')` when `uid === null` —
  `packages/core/src/fiber.ts:224-227`. Parity: carried-verbatim (the
  v1-only stricter "not during UNLOADING drain" gate is port freedom, §12).
- **`internal/listener` is a bail-style veto/replace point on every
  registration** — a bailing observer returns a disposer and the normal
  registration is skipped: `packages/core/src/events.ts:152-153`. Parity:
  carried (v1's `on_internal` vocabulary).
- **once removes itself, then runs the listener, only when invoked**:
  wrapper is `function (...args) { dispose(); return listener.apply(this,
  args) }` — `packages/core/src/events.ts:160-166`; spec pins single
  delivery + manual dispose (`packages/core/tests/events.spec.ts:31-42`).
  Each `once` removes only its own wrapper, so unreached once-listeners stay
  registered — exactly v1's claim-at-invocation fix
  (`DESIGN.zh-CN.md:1050-1067`). Parity: carried-verbatim.
- **Carrier filtering (thisArg + `[Context.filter]`)**: when args[0] is an
  object/function it is the thisArg; hooks are admitted iff `hook.global ||
  !filter || filter.call(thisArg, hook.ctx)` —
  `packages/core/src/events.ts:72-81`. No thisArg ⇒ no filtering (the `!filter`
  arm). The default service filter is isolate-key equality —
  `packages/core/src/service.ts:37-39`; a scoped emit derives a ctx carrying
  the filter (`Object.create` + `[symbols.filter]`) —
  `packages/core/src/reflect.ts:221-225`. Parity: carried (v1's
  `EventCarrier` + scope-chain reachability, `DESIGN.zh-CN.md:449-468`, is
  the port's re-expression; upstream's filter is an arbitrary predicate, v1
  restricts to scope-chain prefix matching — carried-with-rework).
- **`internal/dispatch` audit**: every non-`internal/*` dispatch emits
  `internal/dispatch (mode, name, args, thisArg)` when observed —
  `packages/core/src/events.ts:75-77`. Parity: carried.

## 3. Fiber lifecycle, effects, disposal

- **Fiber is the upstream public handle; there is no Fork symbol anywhere**
  (grep over `packages/*/src` at this rev: zero matches). The handle even
  doubles as a `PromiseLike`: `plugin()` returns `Object.create(fiber)` with
  `then` delegating to `fiber.await()` —
  `packages/core/src/registry.ts:207-212`. Parity: port freedom
  (Fork/Fiber split, `DESIGN.zh-CN.md:299`) — direction endorsed by upstream
  shape (the promise-mixin is JS-only).
- **Per-fiber derived context**: every plugin fiber's ctx is
  `parent.extend({ fiber: this })` — `packages/core/src/fiber.ts:135`. This
  is the upstream ancestor chain that v1 reproduces with per-fiber scope
  nodes (`DESIGN.zh-CN.md:455-461`). Parity: carried-with-rework.
- **Inject config becomes the fiber's own intercept layer**: each non-null
  inject entry writes `ctx[Context.intercept][name] = config` on a
  prototype-chained overlay created at spawn —
  `packages/core/src/fiber.ts:137-144`. (v1 cites exactly these lines,
  `DESIGN.zh-CN.md:869` — verified correct.) Parity: carried-verbatim.
- **States**: `PENDING, LOADING, ACTIVE, FAILED, DISPOSED, UNLOADING` —
  `packages/core/src/fiber.ts:78-85`; derived state is a pure function of
  uid/error/epoch — `fiber.ts:348-353` (uid null ⇒ DISPOSED; `_error` ⇒
  FAILED; epoch ≠ INACTIVE ⇒ ACTIVE; else PENDING). Parity:
  carried-verbatim (v1 collapses the loading/unloading wings into the
  inertia tri-state — carried-with-rework, see §4).
- **Typed framework error**: `CordisError` with a code enum;
  `INACTIVE_EFFECT: 'cannot create effect on inactive context'` —
  `packages/core/src/fiber.ts:87-99`. Parity: carried (v1's
  `CordisError::Code` family extends the code set — port freedom for the
  additional codes).
- **Effect model**: an effect is a disposer, an iterable of disposers, a
  promise of one, or an (async) iterable that streams disposers —
  `packages/core/src/fiber.ts:48-64,229-273`. Async-iterator effects stop
  collecting when the epoch changed mid-flight (`fiber.ts:263`). Disposers
  run LIFO: `.splice(0).reverse()` — `fiber.ts:283`; `DisposableList.clear()`
  also returns reversed — `packages/core/src/utils.ts:26-30`. Parity:
  carried-verbatim (LIFO discipline; generator-composition is JS-shaped —
  v1's `compose_effects`/`adopt` are the port re-expression,
  `DESIGN.zh-CN.md:483-491`).
- **Effect setup failure rolls back**: a throwing `execute` runs the already
  collected disposers, then rethrows — `packages/core/src/fiber.ts:311-316`.
  Parity: carried-verbatim (v1 P2 #2).
- **Dispose-at-most-once per effect**: an epoch flag guards the wrapper —
  `packages/core/src/fiber.ts:322-332`; unhandled rejections are contained
  (`fiber.ts:318-320`).
- **Effect metadata**: `{ label, children }` meta tree attached via
  `symbols.effect` — `packages/core/src/fiber.ts:66-69,296-308,342-346`.
  Parity: carried (v1 sixth-pass #12's nested `metas()`).
- **Registration permanence (strong reference)**: runtimes hold fibers in a
  `DisposableList<Fiber>` — `packages/core/src/registry.ts:203`; spawn pushes
  `this` — `packages/core/src/fiber.ts:170-171`. No external handle is
  required for a fiber to stay alive. Parity: carried-verbatim
  (`DESIGN.zh-CN.md:757-770`, v1's `Arc<Fiber>`).
- **…but upstream couples child lifetime to the parent**: the spawn itself
  is registered as a *parent* effect labeled `'ctx.plugin()'`; parent
  teardown removes the child from `runtime.fibers` —
  `packages/core/src/fiber.ts:170-199` (effect at `:170`, label at `:199`).
  v1 deliberately drops the cascade (dynamic spawn survives parent dispose;
  omission ledger §13, `DESIGN.zh-CN.md:719`). Parity:
  deliberately-omitted (documented).
- **Death mark precedes drain**: the spawn-effect teardown sets
  `this.uid = null` first (`packages/core/src/fiber.ts:180`), emits
  `internal/plugin`, deregisters, `_setEpoch(INACTIVE)` (`:188`), and only
  then awaits inertia to drain (`while (this.inertia) await this.inertia`,
  `:195-197`). This is the upstream fact v1's "死亡标记前置" aligns to
  (`DESIGN.zh-CN.md:890-916`). Parity: carried-verbatim.
- **Disposer failures are contained per-disposer (log and continue)**:
  `_unload` wraps each dispose in `composeError`, catches, and reports via
  `ctx.logger.error` — `packages/core/src/fiber.ts:437-448`. Parity:
  carried-verbatim (v1's catch_unwind containment, `DESIGN.zh-CN.md:950-955`).
- **`await()` waits out inertia, then throws the raw error or returns the
  fiber** — `packages/core/src/fiber.ts:460-466`. Parity: carried-with-rework
  (v1's `ready() -> Result<FiberState>` types the outcome — port freedom).
- **`restart()`** = assertActive, `_setEpoch(INACTIVE)`, `_refresh()`,
  await — `packages/core/src/fiber.ts:468-474`. Parity: carried.
- **`update(config, noSave)` is a vetoable waterfall**: validates config,
  then `waterfall(fiber, 'internal/update', config, noSave, next)` where
  `next` performs the config swap and `restart()` —
  `packages/core/src/fiber.ts:476-485`. A layer that never calls next vetoes
  the swap (implicit short-circuit, §2). Parity: carried-verbatim
  (v1's `Fork::update`; the fallible-tail refinement is v1's).
- **Name resolution walks ancestors to root** —
  `packages/core/src/fiber.ts:215-222`.
- **`DisposableList`** (identity-keyed, sn-ordered, reversible clear) —
  `packages/core/src/utils.ts:4-39`. Parity: carried-verbatim
  (label-taking push is v1's hardening).
- **`List<T>`** (effect-scoped collection; push registers `ctx.effect`,
  sn-keyed map, generator filter/map) — `packages/utils/src/index.ts:4-42`.
  Parity: carried-verbatim (v1 `cordis-core/src/list.rs`;
  `DESIGN.zh-CN.md` P2 #5; upstream `trace` param is the tracer v1 skips).

## 4. Settle protocol: epoch and inertia

- **Epoch is a fingerprint of provider fiber uids**: `_refresh` builds
  `epoch = '' + ':' + impl.fiber.uid` per inject entry, breaking to the
  `INACTIVE` sentinel when any required impl is missing —
  `packages/core/src/fiber.ts:385-397`; sentinel `'__INACTIVE__'` at
  `fiber.ts:101`; the epoch cell lives on the runner
  (`EffectRunner<string>`, `fiber.ts:119,71-76`). Root fiber's epoch is
  `''` (never INACTIVE, hence permanently active) — `fiber.ts:206`.
  Parity: carried-verbatim semantics (sequence of provider uids + inactive
  sentinel); `Option<Vec<u64>>` is the Rust encoding — port freedom
  (`DESIGN.zh-CN.md:210-216`).
- **Epoch change during flight is recorded, not acted on**: `_setEpoch`
  writes the cell then returns early if `this.inertia` exists —
  `packages/core/src/fiber.ts:399-403`. The in-flight cycle's tail re-reads
  it. Parity: carried-verbatim.
- **The convergence loop ("re-check at exit")**: `_reload` captures
  `oldEpoch` *before* apply (`fiber.ts:417`), and on completion compares the
  current runner epoch against that captured target — equal ⇒ inertia
  cleared (settled); different ⇒ chain into `_unload` and stay UNLOADING
  (`fiber.ts:427-434`). `_unload` mirrors it against INACTIVE — equal ⇒
  settled, different ⇒ chain into `_reload` (`fiber.ts:450-457`). So
  flapping services ping-pong until the fingerprint stabilizes. Parity:
  carried-verbatim (`DESIGN.zh-CN.md:217-242`). The *comparison target*
  being the pre-apply snapshot (not live provider state) is exactly v1
  fifth-round #1's fix (`DESIGN.zh-CN.md:745-755`) — verified anchored at
  `fiber.ts:417,427-434`.
- **`_setEpoch` routes by sentinel crossing, not by direction**: entering
  non-INACTIVE from INACTIVE ⇒ `_reload` + LOADING; otherwise ⇒ `_unload` +
  UNLOADING — `packages/core/src/fiber.ts:404-412`. Parity: carried.
- **Failed apply upstream**: catch logs, sets `_error`, and **resets the
  epoch cell to INACTIVE** so the tail unloads —
  `packages/core/src/fiber.ts:421-426`. v1 diverges deliberately (§1.2).
- **A same-fingerprint epoch write is a no-op** — `if (epoch === oldEpoch)
  return` (`packages/core/src/fiber.ts:400-401`). This is the upstream half
  of v1's "unchanged notifications are absorbed" argument.
- **Inertia is "a cycle promise exists"**: `inertia: Promise<void> |
  undefined` — `packages/core/src/fiber.ts:110`; created/cleared only inside
  `_setEpoch`/`_reload`/`_unload` tails. v1's IDLE/ACTIVE/RELEASING
  tri-state + "ready() only acknowledges IDLE"
  (`DESIGN.zh-CN.md:1124-1128`) is a typed re-expression: port freedom
  anchored at `fiber.ts:110,399-458,460-466`.
- **Store snapshot at reload**: `this.store = { ...this._store }` before
  apply (`packages/core/src/fiber.ts:416`), `undefined` after unload
  (`fiber.ts:449`) — the applied-vs-visible service set distinction.
- **Store contents are gated by `_checkImpl`**: present-and-passing impls
  only; a failing `check` or an error deletes the entry —
  `packages/core/src/fiber.ts:371-383`. Parity: carried (v1 keeps the
  ACTIVE gate, omits the arbitrary predicate — §13).

## 5. Services, visibility, notification

- **Strict visibility requires the provider to be ACTIVE**:
  `_getImpl(name, strict = true)` returns nothing when
  `impl.fiber.state !== FiberState.ACTIVE` —
  `packages/core/src/reflect.ts:154-160`. Parity: carried-verbatim
  (`DESIGN.zh-CN.md:722-730`).
- **ACTIVE transitions notify dependents**: `_updateState` walks the fiber's
  own impls and `reflect.notify`s them, but only on crossings between
  ACTIVE and non-ACTIVE — `packages/core/src/fiber.ts:355-369` (emit
  `internal/status` at `:360`, notify at `:362-368`). Parity:
  carried-verbatim (v1's "settle 进入 ACTIVE 时向依赖方广播").
- **Provider broadcasts, consumers are passive**: `notify(names, filter)`
  iterates `registry.values() → runtime.fibers`, and for each fiber that
  declares the inject and whose isolate key matches, runs `_checkImpl` +
  `_refresh` — `packages/core/src/reflect.ts:205-220`. Consumers never poll.
  Parity: carried-verbatim (`DESIGN.zh-CN.md:244-255`). Call sites:
  `provide` when ACTIVE (`reflect.ts:192-194`), the unprovide disposer
  (`reflect.ts:195-201`), and fiber state transitions (`fiber.ts:362-368`).
- **Unprovide waits for dependents before removing self**: the provide
  disposer deletes the store entry, notifies, `await
  Promise.allSettled(fibers.map(f => f.await()))`, and only then drops the
  fiber's own store reference ("ensure self access before dependencies
  cleanup") — `packages/core/src/reflect.ts:195-201`. Parity: carried.
- **`provide` is an effect with duplicate detection**: registers
  `{ type: 'service' }`, interns the root isolate symbol, throws
  `` service "name" has been registered at <fiber> `` on collision, stores
  the impl, and notifies if already ACTIVE —
  `packages/core/src/reflect.ts:175-203` (root symbol at `:184-185`,
  duplicate at `:187-189`, label `` `ctx.provide(${name})` `` at `:202`).
  Parity: carried-verbatim (v1 `DuplicateService`).
- **The unmapped name resolves to the root slot**: `ctx.root[symbols.isolate]
  [name] ??= Symbol(name)` — `packages/core/src/reflect.ts:184-185`; the
  prototype chain of isolate dicts bottoms out at the constructor's
  `Object.create(null)` root layer — `packages/core/src/context.ts:37`.
  Parity: carried (v1 `IsolateId(0)` global slot; counter-based ids are
  port freedom).
- **`set` is restricted to the providing fiber**: no impl ⇒ "cannot set
  property without provide"; different fiber ⇒ "cannot set property in
  multiple fibers" — `packages/core/src/reflect.ts:162-173`. Parity:
  carried.
- **Service reads outside inject are errors by default**: the Proxy get trap
  throws "cannot get property without inject", with the `internal/get`
  waterfall as the interception point — `packages/core/src/reflect.ts:71-94`;
  the walk up the fiber chain honors isolate-key continuity (`:81-93`).
  `internal/set` analogous — `reflect.ts:105-124`. Parity:
  deliberately-omitted in v1 (Proxy-anchored; §13).
- **`Service` base**: constructor auto-provides under the class name, wires
  the `[symbols.filter]` isolate-equality default —
  `packages/core/src/service.ts:18-39`; callable-service and mixin machinery
  at `service.ts:26-49`. Config interception layers merge
  prototype-outwards — `service.ts:51-67`.

## 6. Context, scope, isolation

- **Derivation is prototype-chain `Object.create`, O(1) per derive,
  O(depth) per read, nearest layer wins**: `extend(meta)` —
  `packages/core/src/context.ts:55-63`; `isolate(name, label?)` shadows one
  key — `context.ts:65-69`; `intercept(name, config)` —
  `context.ts:71-77`. Parity: carried-verbatim *semantics* (v1's
  cheap-clone handle + layered nodes is the Rust encoding,
  `DESIGN.zh-CN.md:295-391`; v1's rejection of COW flattening is about the
  encoding only).
- **`isolate(name, label)` is dual-realm**: no label ⇒ fresh `Symbol(name)`
  (private); label ⇒ that symbol shared by identity (shared realm) —
  `packages/core/src/context.ts:65-69`. Parity: carried (v1
  `IsolateRealm::{Private, Shared}`).
- **Context is a Proxy** (property injection, shadow tracking) —
  `packages/core/src/context.ts:36-49` and the handler at
  `packages/core/src/reflect.ts:62-133`. Parity: deliberately-omitted
  (JS property system; v1 explicit `(IsolateId, NAME)` lookup).
- **`ctx.root` is the live root context** — assigned in the constructor
  (`this.root = self`, `packages/core/src/context.ts:40`), with the root
  fiber created there (`context.ts:42`) and permanently ACTIVE
  (`fiber.ts:200-212`). Parity: carried-verbatim (v1's `Context::root()`
  live handle, `DESIGN.zh-CN.md:306`).
- **Realms in the loader**: `LocalRealm` (suffix `#entryId`, per-entry) and
  `GlobalRealm` (suffix `@label`, shared) —
  `packages/loader/src/config/isolate.ts:47-65`; label-keyed interning
  `realms[label] ??= new GlobalRealm(label)` — `isolate.ts:67-85`; the
  `patch-context` flow builds a new isolate map as one layered node
  (`newMap = Object.create(parent)`, then per-name access) —
  `isolate.ts:92-97`; realm GC on partial dispose — `isolate.ts:151-168`.
  Parity: carried-with-rework (v1 `with_isolate_realms` mirrors the newMap
  shape and GlobalRealm intern, `DESIGN.zh-CN.md:801-819`; note upstream GC,
  §1.6, and that upstream keeps this in the loader package, not core).

## 7. Registry

- **Per-runtime counter, first fiber uid = 1**: `_counter = 0`, `counter`
  returns `++this._counter` — `packages/core/src/registry.ts:126,136-138`;
  root fiber separately owns uid 0 (`fiber.ts:201`). Parity:
  carried-verbatim (`DESIGN.zh-CN.md:308-349`).
- **Runtime is shared per plugin callback**: `plugin()` get-or-creates the
  runtime keyed by the resolved callback —
  `packages/core/src/registry.ts:193-205`; each call spawns a new fiber on
  that runtime (`registry.ts:207`). No dedup question exists upstream: one
  runtime, N fibers. Parity: v1's per-spawn `Anonymous(n)` runtimes +
  `prune_if_idle` is carried-with-rework (see §1.1 for the corrected
  framing).
- **Re-registration after delete recreates the runtime** —
  `packages/core/src/registry.ts:199-205`. Parity: carried (v1 sixth-pass
  #1's "new generation runtime" alignment).
- **`delete(plugin)` disposes every fiber of the runtime** —
  `packages/core/src/registry.ts:162-171`. Parity: carried (v1
  `Registry::remove`, infallible variant).
- **Registry introspection**: `keys/values/entries/forEach/size` —
  `packages/core/src/registry.ts:136-187`. Parity: carried (v1 P2 #4
  `records()/contains()`).
- **`Inject.resolve` merges prototype-then-own, later writes win** —
  `packages/core/src/registry.ts:43-60`; the loader re-resolves entry inject
  over fiber inject at spawn — `packages/loader/src/index.ts:88-94`.
  Parity: carried-verbatim (v1 sixth-pass #3 `require_overriding`,
  `DESIGN.zh-CN.md:1101-1104`).

## 8. Logger

- **No default buffer exporter is NOT upstream — upstream installs one**:
  `bufferSize = 1000` and a built-in exporter that trims the ring —
  `packages/core/src/logger.ts:171-172,189-201`. v1 deliberately omits it
  (§13, `DESIGN.zh-CN.md:720`). Parity: deliberately-omitted (v2 keeps
  the omission; the `logging_exporters` tour demonstrates it —
  pre-registration records retained nowhere, `BufferExporter` is opt-in).
- **Exporter registration is an effect** labeled `'ctx.logger.exporter()'`
  — `packages/core/src/logger.ts:206-212`. Parity: carried-verbatim (the
  tour's lifecycle beat: dispose detaches the exporter).
- **Logger name resolution order**: explicit argument → intercept
  `logger.name` → `hyphenate(fiber.name)` —
  `packages/core/src/logger.ts:226-237` (resolveConfig walks the intercept
  prototype chain, `logger.ts:214-224`). v1 carried the full order; v2
  drops the intercept step — no public intercept derive exists in a
  serde-free core (§13 row below): name = explicit argument →
  `hyphenate(fiber.name)`.
- **Level filtering is per exporter**: `exporter.levels[name] ??
  exporter.levels.default ?? logger.level ?? INFO`, skip when target <
  level — `packages/core/src/logger.ts:140-145`. v1 carried the chain
  with a cap-narrowing intercept; v2 carries the per-exporter half
  (`min_level(name)` / `default_level()`) — the `logger.level` fallback
  was scope-intercept state and drops with the §13 row.
- **AggregateError unwrapping in the default path** —
  `packages/core/src/logger.ts:129-136`. Four levels ERROR<WARN<INFO<DEBUG
  (`logger.ts:18-23`), message envelope with `sn/ts/name/type/level/args`
  and a `WeakRef<Fiber>` (`logger.ts:25-33`).

## 9. Timer

Upstream timer is a separate package (`packages/timer/src/index.ts`), a
`Service('timer')` mixing six methods onto ctx —
`packages/timer/src/index.ts:11-15`.

- **Every timer shape is a fiber effect** — timeout callback form
  (`timer/index.ts:33-39`), timeout promise form (`:43-50`), interval both
  forms (`:60-63,67-77`), `_schedule` for throttle/debounce (`:103-108`).
  Parity: carried-verbatim.
- **Disposal cancels pending timers synchronously**: `isDisposed = true;
  clearTimeout(timer)` — `packages/timer/src/index.ts:105-108`; after
  dispose, throttle schedules nothing (`!isDisposed` guard at `:128`) —
  cancellation *closes the window*; a trailing call never fires post-cancel.
  Parity: carried-verbatim (v1's "取消即关窗", `DESIGN.zh-CN.md:569-576`).
- **Throttle is clock-driven with a self-healing window**:
  `remaining = delay - now + lastCall`; ≤ 0 fires now, else schedules the
  trailing edge — `packages/timer/src/index.ts:117-132`; `noTrailing` is the
  third parameter, threaded as the disposed-flag initial value (`:123,131`).
  Parity: carried (v1's `throttle_with(_, _, ThrottleOptions)`; the
  no-clock-fallback rewriting is v1's Rust-timer reality, port freedom).
- **Timeout deadline starts at call** (host `setTimeout` semantics) —
  `packages/timer/src/index.ts:29-39`; v1 aligned its pinning to call time
  (`DESIGN.zh-CN.md:628`). Parity: carried.
- **Upstream timeout has BOTH a callback form and a promise form that
  REJECTS on early dispose** (`Error('Context has been disposed')`) —
  `packages/timer/src/index.ts:27-52` (reject at `:47`). v1 keeps only the
  future form with no early-cancel handle — deliberately-diverged
  (`DESIGN.zh-CN.md:600-605`).
- **Upstream interval-as-async-iterator REJECTS `next()` on context
  dispose** — `done = { kind: 'throw', reason: … }` + reject —
  `packages/timer/src/index.ts:65-83` (dispose path `:71-76`). v1's stream
  terminates instead — deliberately-diverged (`DESIGN.zh-CN.md:577-580`).
- **Upstream drops ticks the consumer isn't awaiting**: the tick handler
  resolves the single pending `nextTask` only (`nextTask?.resolve(...)`,
  `timer/index.ts:68-70`); ticks with nobody parked in `next()` are lost, so
  throughput ≈ 1/max(handle, delay). v1's consumer-paced catch-up —
  deliberately-diverged (`DESIGN.zh-CN.md:582-586`).
- **ArmedSleep / biased-select / triple-redundant cancel layers**: no
  upstream counterpart (JS has no interleaved poll races) — port freedom
  (§12), anchored on the plain `clearTimeout` discipline above.

## 10. Loader

Upstream loader is a full **service** (`Loader extends EntryTree`, provides
`'loader'`) — `packages/loader/src/index.ts:47-72`. v1's core deliberately
keeps only `load(ctx, resolver)` + caller-held fork table and pushes
file/patch/preset composition out (ADR 0003): for this doc that makes most
loader rows carried-with-rework or omitted; the semantic anchors:

- **Disabled reachability is one upward walk, groups always enabled**: `get
  disabled()` returns false for group rows outright, else walks ancestors —
  `packages/loader/src/config/entry.ts:63-73`. Parity: carried-verbatim
  (v1's `is_enabled` single home, arch-review #03;
  `DESIGN.zh-CN.md:532`).
- **Group rows are structural**: `EntryOptions.group` marks them
  (`entry.ts:8-15`); groups skip config interpolation (`_resolveConfig`
  returns raw options for `EntryGroup.key` plugins — `entry.ts:79-81`);
  a `Group` plugin's whole job is managing child entries (init yields a
  stop-disposer, then applies the row list) —
  `packages/loader/src/config/group.ts:73-88`. No isolation is derived from
  a group label (isolation comes solely from `options.isolate`, §6).
  Parity: carried-verbatim (v1 examples-api-tour #03's structural rows).
- **Per-entry failure containment, siblings proceed**: `EntryGroup.update`
  maps all ids through create/remove, catching per-entry errors into the
  logger — `packages/loader/src/config/group.ts:47-64`; import failure in
  `Entry._init` logs and leaves the entry fiber-less —
  `packages/loader/src/config/entry.ts:158-167`. Parity: carried
  (semantics); v1's typed `LoadOutcome { spawned, errors }` returning
  handles is the Rust strengthening (`DESIGN.zh-CN.md:533`).
- **Entry ids are path-shaped with `:` separator; subtrees hang off
  entries** — `packages/loader/src/config/tree.ts:6-7,25-31,56-67`.
- **Tree quiescence is a drain loop over pending tasks** —
  `packages/loader/src/config/tree.ts:33-45`.
- **Entry update diffs by deep-equality and re-fibers via
  `fiber.update(..., true)`** — `packages/loader/src/config/entry.ts:100-134`
  (diff at `:124-130`, patch-context waterfall `:84-92`). The patch/replay
  layer above this (include, hmr) is harness-side and out of scope.
- **File/module resolution is Node-coupled** (`ModuleLoader` internals) —
  `packages/loader/src/internal.ts:1-123`; v1's resolver-injection replaces
  it wholesale — port freedom anchored here.
- **`inject:` on rows re-resolves over plugin inject at spawn** —
  `packages/loader/src/index.ts:88-94` (§7 above).

## 11. internal/* event inventory

The complete upstream internal vocabulary —
`packages/core/src/events.ts:169-178`:

| Event | Shape | v1/v2 status |
|---|---|---|
| `internal/plugin` | `(fiber)` — emitted at spawn-registration and at teardown | carried (`fiber.ts:164,181`) |
| `internal/status` | `(fiber, oldState)` — every state change | carried (`fiber.ts:360`) |
| `internal/service` | `(name, value)`, this: Context with filter — scoped emit | carried (`reflect.ts:221-225`) |
| `internal/update` | `(config, noSave, next)` — per-fiber vetoable onion | carried-with-rework (`fiber.ts:476-485`, `events.ts:54-69`) |
| `internal/get` | `(ctx, name, error, next)` — Proxy get interception | deliberately-omitted (`reflect.ts:80-94`) |
| `internal/set` | `(ctx, name, value, error, next)` — Proxy set interception | deliberately-omitted (`reflect.ts:118-120`) |
| `internal/listener` | `(name, listener, options)` bail — registration veto | carried (`events.ts:152-153`) |
| `internal/dispatch` | `(mode, name, args, thisArg)` — audit emit | carried (`events.ts:75-77`) |

## 12. Port-freedom index

Decisions with **no upstream counterpart** — do not force a citation;
ticket 07 marked these port-freedom pending their validating consumer.
Close-out (core-v2 ticket 27, promotion records below the list): every
anchored class landed green across the six-seat suite; the one item with
no consumer (`compose_effects`/`adopt`) never landed as v2 surface at
all — recorded, not papered over:

- Marker-trait listener typing (`SyncUnit`/`AsyncUnit`/`Mapping`/`Bailing`/
  `AsyncBailing`/`Around`), `Flow<T>`, `around()` wrapper fn, trybuild UI
  gates. Upstream registers untyped closures (`events.ts:144-158`); shapes
  exist only as dispatch modes (`events.ts:14`).
  Validated by every listener in all six seats (tickets 22–27); inference
  carried without annotation beyond the documented residual cases, and the
  trybuild gates pin the muscle-memory failure modes.
- `Next<E>` as consuming `FnOnce` (upstream: unenforced runtime discipline,
  `events.ts:120-125`).
  Validated by the gateway's `Gate` around layers (ticket 23) and the
  chat capstone's Message pipeline — recovery and veto layers (ticket
  27): each continuation called exactly once per layer, by construction.
- `Fork` public handle vs `Fiber` body split (upstream exports only
  `Fiber`, promise-mixed — `registry.ts:207-212`).
  Validated across the suite: every spawn/dispose/restart in all six
  seats drives a `Fork`; no example ever holds a fiber body.
- epoch as `Option<Vec<u64>>` (upstream: string fingerprint —
  `fiber.ts:385-397`); uid `0` as the dead sentinel (upstream: `null`;
  root owns 0 — `fiber.ts:201`).
  Validated by the worker daemon's crash/heal/restart runbook (ticket
  24) — restarts and epoch reloads under failure — plus the settle
  contract rows.
- All lock discipline: no-user-code-in-critical-section, two-phase
  teardown, `parking_lot`, `String`-narrowed `push_with_label`,
  `await_holding_lock` lint, STATE_TABLE exhaustive dispatch. Upstream is
  single-threaded JS — no locks exist.
  Validated by the whole suite running green under the matrix plus the
  race suite (ticket 21); the consumer-visible face is promoted at its
  own home (ADR 0010 decision 6).
- `Registry::attach_fiber` single critical section (linearizing spawn vs
  `remove`); the stricter `assert_can_register` gate rejecting during
  UNLOADING drain (upstream's only gate is `uid === null`,
  `fiber.ts:224-227`).
  Validated by the registry race rows (ticket 21) and the
  `logging_exporters` tour's registry-remove beat (ticket 26).
- `ArmedSleep` primitive: arm-time deadline pinning, biased select, the
  three redundant cancellation layers, off-runtime gating
  (`arm -> None`). Upstream cancellation is one synchronous `clearTimeout`
  (`timer/index.ts:105-108`).
  Validated by its three shape seats: the gateway's timeout deadlines
  (ticket 23) and the worker daemon's crash-loop heal + queue beat
  (ticket 24).
- Loader cycles-impossible-by-construction (insert validates parent only).
  Upstream tree shape is store+group references (`tree.ts:56-81`).
  Validated by the gateway's declarative JSON boot over a multi-row
  config tree (ticket 23).
- `compose_effects`/`adopt`/`EffectBuilder` (upstream composes via
  generator yield positions — `fiber.ts:246-268`).
  **No v2 consumer appeared** (deferred with core-v2 ticket 16, and none
  grew since): the composite-effect API never landed as v2 surface —
  no-consumer-no-API won. Not provisional landed surface; rides as
  harness-era debt to be revisited if a natural consumer grows.
- Introspection seams: `Fork::pending_missing()`,
  `Context::services_snapshot()`, `FiberState` audit side-bands. Upstream
  exposes registry iteration only (`registry.ts:173-187`).
  Validated per ADR 0005 seams 1–2 (tickets 22/24/26; records at that
  home).

## 13. Omissions and divergences

Upstream capabilities v1 chose not to port (or to bend), each with its
anchor — the "deliberate" ledger ticket 07 ratifies. Sources:
`DESIGN.zh-CN.md:708-720` (the omission table) plus divergence records
noted inline above.

| Upstream behavior | Anchor | v1 decision |
|---|---|---|
| `internal/get` / `internal/set` waterfalls | `reflect.ts:80-94,118-120` | not ported (Proxy-anchored) |
| accessor / mixin property machinery | `reflect.ts:229-265`, `service.ts:41-49` | not ported (JS property system) |
| generic `ctx.extend(meta)` prototype extension | `context.ts:55-63` | not ported (named derive methods) |
| child cascade dispose (spawn as parent effect) | `fiber.ts:170-199` | not ported (registration permanence wins; revisit trigger recorded) |
| default 1000-entry ring buffer exporter | `logger.ts:171,189-201` | not ported ("no hidden retention"; v2 keeps it — tour-validated, core-v2 26) |
| scope-intercept logger config (`logger.name`/`logger.level` via the interceptor prototype chain) | `logger.ts:140-145,214-237` | not ported (v2 core is serde-free with no public intercept derive; name = explicit → hyphenated fiber name, level routing per exporter; core-v2 26) |
| arbitrary `impl.check` readiness predicate | `reflect.ts:175`, `fiber.ts:371-383` | partial (ACTIVE gate kept; predicate dropped) |
| failed apply resets epoch to INACTIVE | `fiber.ts:421-426` | diverged (keep failure fingerprint; §1.2) |
| `isBailed` JS-falsy table (`false` = no answer) | `events.ts:6-8` | diverged (`Option`; `Some(false)` answers) |
| emit propagates listener throws | `events.ts:96-99`, spec `:87-90` | v1 sketch logged instead — v2 must decide (§1.3) |
| interval iterator rejects on dispose | `timer/index.ts:71-76` | diverged (stream terminates) |
| unobserved interval ticks dropped | `timer/index.ts:68-70` | diverged (consumer-paced catch-up) |
| timeout callback form + rejecting promise form | `timer/index.ts:27-52` | diverged (future form only, no early-cancel handle) |
| `next()` parameterless + shared mutable args | `events.ts:119-124` | diverged (owned args per call) |
| Loader as a resident service with file/patch/HMR layers | `loader/index.ts:47-72`, `internal.ts` | not ported into core (resolver injection; harness-side) |
| realm garbage collection | `loader/.../isolate.ts:151-168` | not ported (boundedness argument; churn validated by the `scopes_tenants` example — ADR 0006 decision 4) |
| runtime-per-callback sharing | `registry.ts:199-205` | diverged (`Anonymous(n)` per spawn + `prune_if_idle`) |

## 14. Could not verify

- **`cosmokit::hyphenate`** (v1 fifth-pass #9's false-parity site): the
  dependency is not vendored in the clone (no `node_modules` at this rev),
  so its implementation cannot be cited from the pinned tree. Verifiable
  anchors only: imported at `packages/core/src/logger.ts:1`, used at
  `logger.ts:231` as the logger-name fallback. Any v2 parity claim about a
  hyphenate re-implementation must cite the cosmokit source fetched
  separately (npm `cosmokit`) or be marked port freedom.
- **Runtime behavior beyond source reading**: all citations above are
  static (source + upstream's own specs under `packages/core/tests/`); no
  upstream runtime was executed during this research.
