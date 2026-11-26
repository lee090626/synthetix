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
