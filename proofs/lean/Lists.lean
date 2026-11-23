/-
Facts about `sum` over a list of any length (`prices[n]`, issue #182).

The Rust side proves each item with Z3 and then treats `sum(xs)` as one
unknown number. These theorems are what it may assume about that number:
it lies between n * a and n * b when every item does (`sum_bounds`), every
running total does too (`prefix_bounds`), and a loop that wraps each running
total, as emitted code does, gives the exact sum when no running total leaves
the range the wrapping keeps unchanged (`fold_exact`).
-/

theorem sum_bounds (a b : Int) : ∀ xs : List Int, (∀ x ∈ xs, a ≤ x ∧ x ≤ b) →
    xs.length * a ≤ xs.sum ∧ xs.sum ≤ xs.length * b
  | [], _ => by simp
  | x :: xs, h => by
    have hx := h x (by simp)
    have ih := sum_bounds a b xs (fun y hy => h y (by simp [hy]))
    simp only [List.length_cons, List.sum_cons]
    constructor <;> push_cast <;> rw [Int.add_mul, Int.one_mul] <;> omega

/-- Running totals stay between min 0 (n * a) and max 0 (n * b). -/
theorem prefix_bounds (a b : Int) (xs : List Int) (h : ∀ x ∈ xs, a ≤ x ∧ x ≤ b) (k : Nat) :
    min 0 (xs.length * a) ≤ (xs.take k).sum ∧ (xs.take k).sum ≤ max 0 (xs.length * b) := by
  have ht := sum_bounds a b (xs.take k) (fun y hy => h y (List.mem_of_mem_take hy))
  have hl : (xs.take k).length ≤ xs.length := List.length_take_le' k xs
  have hlo : min 0 (xs.length * a) ≤ (xs.take k).length * a := by
    rcases Int.le_total 0 a with ha | ha
    · have : (0 : Int) ≤ (xs.take k).length * a := Int.mul_nonneg (by omega) ha
      omega
    · have : (xs.length : Int) * a ≤ (xs.take k).length * a :=
        Int.mul_le_mul_of_nonpos_right (by omega) ha
      omega
  have hhi : (xs.take k).length * b ≤ max 0 (xs.length * b) := by
    rcases Int.le_total 0 b with hb | hb
    · have : ((xs.take k).length : Int) * b ≤ xs.length * b :=
        Int.mul_le_mul_of_nonneg_right (by omega) hb
      omega
    · have : ((xs.take k).length : Int) * b ≤ 0 := Int.mul_nonpos_of_nonneg_of_nonpos (by omega) hb
      omega
  omega

/-- A loop adding items with a wrapping step `f` gives the exact sum when `f`
leaves every running total unchanged. -/
theorem fold_exact (f : Int → Int) (R : Int → Prop) (hf : ∀ y, R y → f y = y) :
    ∀ (xs : List Int) (s : Int), (∀ k, R (s + (xs.take k).sum)) →
    xs.foldl (fun acc x => f (acc + x)) s = s + xs.sum
  | [], s, _ => by simp
  | x :: xs, s, h => by
    have h1 : f (s + x) = s + x := hf _ (by simpa using h 1)
    simp only [List.foldl_cons, h1, List.sum_cons]
    rw [fold_exact f R hf xs (s + x) (fun k => by simpa [Int.add_assoc] using h (k + 1))]
    omega

/-- Emitted code: wrapping to `lo..hi` gives the exact sum when n * a and
n * b both fit. -/
theorem loop_sum_exact (f : Int → Int) (lo hi a b : Int)
    (hf : ∀ y, lo ≤ y → y ≤ hi → f y = y) (xs : List Int)
    (h : ∀ x ∈ xs, a ≤ x ∧ x ≤ b) (hlo : lo ≤ min 0 (xs.length * a))
    (hhi : max 0 (xs.length * b) ≤ hi) :
    xs.foldl (fun acc x => f (acc + x)) 0 = xs.sum := by
  have := fold_exact f (fun y => lo ≤ y ∧ y ≤ hi) (fun y hy => hf y hy.1 hy.2) xs 0
    (fun k => by have := prefix_bounds a b xs h k; simp only [Int.zero_add]; omega)
  simpa using this

/-- `min(xs)` and `max(xs)` (issue #190): a loop that starts at the first item
and keeps one of its two numbers each step (`min`, `max`) ends between a and b
when every item does. -/
theorem pick_bounds (f : Int → Int → Int) (hf : ∀ x y, f x y = x ∨ f x y = y) (a b : Int) :
    ∀ (x : Int) (xs : List Int), (∀ y ∈ x :: xs, a ≤ y ∧ y ≤ b) →
    a ≤ xs.foldl f x ∧ xs.foldl f x ≤ b
  | x, [], h => h x (by simp)
  | x, y :: ys, h => by
    simp only [List.foldl_cons]
    apply pick_bounds f hf a b (f x y) ys
    intro z hz
    rcases List.mem_cons.mp hz with rfl | hz
    · rcases hf x y with e | e <;> rw [e]
      · exact h x (by simp)
      · exact h y (by simp)
    · exact h z (by simp [hz])

theorem min_bounds (a b x : Int) (xs : List Int) (h : ∀ y ∈ x :: xs, a ≤ y ∧ y ≤ b) :
    a ≤ xs.foldl min x ∧ xs.foldl min x ≤ b :=
  pick_bounds min (fun x y => by omega) a b x xs h

theorem max_bounds (a b x : Int) (xs : List Int) (h : ∀ y ∈ x :: xs, a ≤ y ∧ y ≤ b) :
    a ≤ xs.foldl max x ∧ xs.foldl max x ≤ b :=
  pick_bounds max (fun x y => by omega) a b x xs h

/-- `min(xs)` is at most every item, so at most `max(xs)` (`max_ge`). -/
theorem min_le (x : Int) : ∀ (xs : List Int), ∀ y ∈ x :: xs, xs.foldl min x ≤ y
  | [], y, hy => by simp at hy; simp [hy]
  | z :: zs, y, hy => by
    simp only [List.foldl_cons]
    have hm := min_le (min x z) zs (min x z) (by simp)
    rcases List.mem_cons.mp hy with h | hy
    · omega
    rcases List.mem_cons.mp hy with h | hy
    · omega
    exact min_le (min x z) zs y (by simp [hy])

theorem max_ge (x : Int) : ∀ (xs : List Int), ∀ y ∈ x :: xs, y ≤ xs.foldl max x
  | [], y, hy => by simp at hy; simp [hy]
  | z :: zs, y, hy => by
    simp only [List.foldl_cons]
    have hm := max_ge (max x z) zs (max x z) (by simp)
    rcases List.mem_cons.mp hy with h | hy
    · omega
    rcases List.mem_cons.mp hy with h | hy
    · omega
    exact max_ge (max x z) zs y (by simp [hy])

theorem min_le_max (x : Int) (xs : List Int) : xs.foldl min x ≤ xs.foldl max x := by
  have := min_le x xs x (by simp)
  have := max_ge x xs x (by simp)
  omega
