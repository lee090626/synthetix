/-
Repeated steps (`repeat years`, issue #214).

A `repeat` rule is split into a start rule and a step rule, each proven on its
own: every promise holds for the start's answer, and if a promise holds for
`value`, it holds for the step's answer. Emitted code runs the start, then the
step `years` times. This theorem is why every promise then holds for the final
answer, for any number of steps.
-/

/-- The loop emitted code runs: `v := start; repeat n times: v := step v`. -/
def runLoop {α : Type} (step : α → α) : Nat → α → α
  | 0, v => v
  | n + 1, v => runLoop step n (step v)

/-- If `P` holds at the start and every step keeps it, it holds for what the
loop returns after any number of steps. -/
theorem loop_inv {α : Type} (P : α → Prop) (step : α → α)
    (hs : ∀ v, P v → P (step v)) : ∀ (n : Nat) (v : α), P v → P (runLoop step n v)
  | 0, _, h => h
  | n + 1, v, h => loop_inv P step hs n (step v) (hs v h)

/-!
A rule that calls itself once less (`f(n - 1)`, issue #225) is split into a
start rule (the answer for 0) and a step rule taking `k` and the answer for
`k`, giving the answer for `k + 1`. Emitted code runs the start, then the step
for `k` = 0, 1, ... n - 1.
-/

/-- The loop emitted code runs: `v := start; for k in from..from+n: v := step k v`. -/
def runFrom {α : Type} (step : Nat → α → α) : Nat → Nat → α → α
  | _, 0, v => v
  | k, n + 1, v => runFrom step (k + 1) n (step k v)

/-- If `P 0` holds at the start and the step takes `P k` to `P (k + 1)`, then
`P n` holds for what the loop returns after `n` steps from 0. -/
theorem rec_inv {α : Type} (P : Nat → α → Prop) (step : Nat → α → α)
    (hs : ∀ k v, P k v → P (k + 1) (step k v)) :
    ∀ (n k : Nat) (v : α), P k v → P (k + n) (runFrom step k n v)
  | 0, _, _, h => h
  | n + 1, k, v, h => by
    have := rec_inv P step hs n (k + 1) (step k v) (hs k v h)
    rw [show k + (n + 1) = k + 1 + n by omega]
    exact this
