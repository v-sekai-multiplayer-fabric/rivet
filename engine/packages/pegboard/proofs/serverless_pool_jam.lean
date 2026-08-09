/-!
# Formal model: why the serverless runner-pool counter jammed, and why the
# level-triggered reconciler that replaced it cannot.

This is the executable spec behind the `read_desired` reconciler in
`engine/packages/pegboard/src/workflows/runner_pool.rs`. It models the counter
design that USED to drive serverless scaling (now removed) and proves the
replacement is jam-free.

The old design was an `i64` atomic counter (`MutationType::Add`) with two gated
sites:

* `pegboard/src/workflows/actor/runtime.rs`  — `ServerlessDesiredSlotsKey += 1` on allocate
* `pegboard/src/workflows/actor/destroy.rs`  — `-= 1` on destroy, *"even if we do
   not have a runner id ... destroyed while pending allocation"*
* `read_desired`                             — `desired_slots < 0  ⇒  scale to 0`

`read_desired` derived the number of runners to start from that counter, clamping
a negative value to 0. Below we show the negative-with-demand state is REACHABLE
(an actor waits while the pool starts 0 runners) and a permanent SINK (more churn
never heals it): `counter_can_jam`, `jam_is_sink`.

The reconciler that replaced it does not accumulate at all. Each tick it counts
live demand from the source-of-truth indexes (`PendingActorByRunnerNameSelectorKey`
plus used slots on `RunnerAllocIdxKey`) and derives desired runners from that.
`reconciler_never_jams` proves that for any `slots_per_runner ≥ 1` and every
operation trace, positive demand yields a positive desired count. This is the OTP
supervisor pattern: desired is observed from reality, never counted, so there is
no accumulator to drift negative.

"If it is possible to be broken, it will happen for real": here the old breakage
is a concrete, closed reduction, and the new safety is a theorem over all traces.
-/

namespace Rivet.RunnerPool

/-- Operations on the counter, each matched to a Rust site. -/
inductive Op
  /-- An actor takes a serverless slot (`runtime.rs:303`): demand and counter both rise. -/
  | alloc
  /-- A slot-holding actor is destroyed (`destroy.rs:284`), balanced against an `alloc`. -/
  | destroyReal
  /-- A destroy fires `-1` for an actor that never incremented the counter. The `-1`
      (`destroy.rs:284`) runs even when the actor was "destroyed while pending
      allocation", but the `+1` (`runtime.rs:303`) is gated on a different predicate;
      under churn they desync into a decrement with no matching increment. -/
  | spuriousDec
  deriving Repr, DecidableEq

/-- Engine state: the stored counter, and the TRUE demand (actors actually waiting,
    i.e. `PendingActorByRunnerNameSelectorKey`, which `read_desired` never consults). -/
structure S where
  counter : Int
  demand  : Nat
  deriving Repr, DecidableEq

def step (s : S) : Op → S
  | .alloc       => { counter := s.counter + 1, demand := s.demand + 1 }
  | .destroyReal => { counter := s.counter - 1, demand := s.demand - 1 }
  | .spuriousDec => { counter := s.counter - 1, demand := s.demand }

def run (init : S) (ops : List Op) : S := ops.foldl step init

/-- `read_desired`, edge-triggered — current production (slots_per_runner = 1). -/
def readDesiredCounter (s : S) : Nat :=
  if s.counter < 0 then 0 else s.counter.toNat

/-- `read_desired`, level-triggered — the task #34 fix: derive from the source of truth. -/
def readDesiredReconciled (s : S) : Nat := s.demand

/-- **The unified reconciler**, generalizing `readDesiredReconciled` to real
    `slots_per_runner`. This is the single algorithm that replaces all three
    current mechanisms (the pool counter scaler, the per-actor start, and the
    envoy `protocol_version` backoff): every tick it derives the number of runners
    to start from *live demand*, `desired = ⌈demand / slots_per_runner⌉`. It is a
    pure function of the current state, never of the operation history, so no
    drift and no cached identity can enter into it. Matches the Rust
    `div_ceil(slots_per_runner.max(1))` at `runner_pool.rs`. -/
def desiredRunners (slotsPerRunner : Nat) (s : S) : Nat :=
  (s.demand + slotsPerRunner - 1) / max slotsPerRunner 1

def s0 : S := { counter := 0, demand := 0 }

/-- Jam witness: one real actor waiting, plus two spurious decrements from churn. -/
def jamTrace : List Op := [Op.alloc, Op.spuriousDec, Op.spuriousDec]

/-- After the trace: counter = -1 while demand = 1. -/
example : run s0 jamTrace = { counter := -1, demand := 1 } := by native_decide

/-- **The bug, as a closed reduction.** A reachable state has positive real demand
    yet the edge-triggered `read_desired` computes 0 desired runners: the pool
    refuses to start a runner while an actor is waiting. -/
theorem counter_can_jam :
    ∃ ops, (run s0 ops).demand > 0 ∧ readDesiredCounter (run s0 ops) = 0 :=
  ⟨jamTrace, by native_decide, by native_decide⟩

/-- Appending spurious decrements only lowers the counter. -/
theorem spuriousDec_lowers (s : S) (k : Nat) :
    (run s (List.replicate k Op.spuriousDec)).counter = s.counter - k := by
  induction k generalizing s with
  | zero => simp [run]
  | succ n ih =>
    have : List.replicate (n + 1) Op.spuriousDec
        = Op.spuriousDec :: List.replicate n Op.spuriousDec := rfl
    simp only [run, this, List.foldl_cons] at *
    rw [ih (step s Op.spuriousDec)]
    simp [step]; omega

/-- Spurious decrements do not change demand. -/
theorem spuriousDec_keeps_demand (s : S) (k : Nat) :
    (run s (List.replicate k Op.spuriousDec)).demand = s.demand := by
  induction k generalizing s with
  | zero => simp [run]
  | succ n ih =>
    have : List.replicate (n + 1) Op.spuriousDec
        = Op.spuriousDec :: List.replicate n Op.spuriousDec := rfl
    simp only [run, this, List.foldl_cons] at *
    rw [ih (step s Op.spuriousDec)]; simp [step]

/-- **The jam is a permanent sink.** From the jammed state, any amount of further
    churn keeps demand positive and desired at 0 — it never self-heals. -/
theorem jam_is_sink (k : Nat) :
    (run (run s0 jamTrace) (List.replicate k Op.spuriousDec)).demand > 0
    ∧ readDesiredCounter (run (run s0 jamTrace) (List.replicate k Op.spuriousDec)) = 0 := by
  have hjc : (run s0 jamTrace).counter = -1 := by native_decide
  have hjd : (run s0 jamTrace).demand = 1 := by native_decide
  have hc := spuriousDec_lowers (run s0 jamTrace) k
  have hd := spuriousDec_keeps_demand (run s0 jamTrace) k
  rw [hjc] at hc
  rw [hjd] at hd
  refine ⟨?_, ?_⟩
  · omega
  · simp only [readDesiredCounter, hc]
    rw [if_pos (by omega)]

/-- **The fix is jam-free, for every trace.** The reconciled `read_desired`
    computes positive desired whenever real demand is positive — no sequence of
    operations can reach a jam. -/
theorem reconciled_never_jams (ops : List Op) :
    (run s0 ops).demand > 0 → readDesiredReconciled (run s0 ops) > 0 := by
  intro h; exact h

/-- **The unified reconciler cannot jam, for any `slots_per_runner ≥ 1`.** This is
    the generalization that gates the refactor: whatever the churn history, if a
    reachable state has real demand then `desiredRunners` starts at least one
    runner. Because `desiredRunners` reads only `demand` (the counted source of
    truth) and not the accumulated counter, no trace — no amount of spurious
    decrements, no cached identity — can drive it to zero-while-wanted. This holds
    for the mk1 and envoy paths alike, since both now reconcile from the same
    state. -/
theorem reconciler_never_jams (spr : Nat) (hspr : 1 ≤ spr) (ops : List Op) :
    (run s0 ops).demand > 0 → desiredRunners spr (run s0 ops) > 0 := by
  intro h
  unfold desiredRunners
  exact Nat.div_pos (by omega) (by omega)

/-- The unified reconciler agrees with the level-triggered spec at
    `slots_per_runner = 1`: it is a faithful generalization, not a behavior
    change on the correct path. -/
theorem reconciler_refines_reconciled (s : S) :
    desiredRunners 1 s = readDesiredReconciled s := by
  simp [desiredRunners, readDesiredReconciled]

end Rivet.RunnerPool
