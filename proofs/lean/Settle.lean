/-
`settle` brings every balance to 0.

The Rust side (`settle_rules` in crates/axiom-parser/src/simple.rs) has Z3 check
each transfer's range and overflow for a fixed group size, and requires the
balances to sum to 0 (the `o.sum = 0` below). This file proves, for any number
of people, that the transfers it computes cancel every balance exactly.

Person i (0-based) owes `pos o[i]` or is owed `neg o[i]`. Debtors are laid out
on a line in list order (D k = what the first k people owe), creditors likewise
(C k). Person i sends person j the overlap of [D i, D (i+1)] and [C j, C (j+1)]:
`settle_N_(i+1)_(j+1)` computes exactly `ov` below.
-/

def pos (x : Int) : Int := max x 0
def neg (x : Int) : Int := max (-x) 0

/-- Sum of the first k widths. -/
def P (ws : List Int) (k : Nat) : Int := ((ws.take k)).sum

def D (o : List Int) (k : Nat) : Int := P (o.map pos) k
def C (o : List Int) (k : Nat) : Int := P (o.map neg) k

/-- Overlap of [a, b] and [c, d], or 0. -/
def ov (a b c d : Int) : Int := max 0 (min b d - max a c)

/-- What person i sends person j. -/
def t (o : List Int) (i j : Nat) : Int := ov (D o i) (D o (i + 1)) (C o j) (C o (j + 1))

def sent (o : List Int) (i : Nat) : Int := ((List.range o.length).map (t o i)).sum
def recv (o : List Int) (j : Nat) : Int := ((List.range o.length).map (fun i => t o i j)).sum

/-- Overlap of [a, b] with consecutive intervals of widths `ws` starting at s. -/
def cov (a b s : Int) : List Int → Int
  | [] => 0
  | w :: ws => ov a b s (s + w) + cov a b (s + w) ws

theorem cov_zero (a b : Int) : ∀ (ws : List Int) (s : Int), (∀ w ∈ ws, 0 ≤ w) → b ≤ s →
    cov a b s ws = 0
  | [], _, _, _ => rfl
  | w :: ws, s, hw, hb => by
    have h0 : 0 ≤ w := hw w (by simp)
    have := cov_zero a b ws (s + w) (fun x hx => hw x (by simp [hx])) (by omega)
    simp only [cov, ov, this]
    omega

theorem cov_shift (a b : Int) : ∀ (ws : List Int) (s : Int), (∀ w ∈ ws, 0 ≤ w) → a ≤ s →
    cov a b s ws = cov s b s ws
  | [], _, _, _ => rfl
  | w :: ws, s, hw, ha => by
    have h0 : 0 ≤ w := hw w (by simp)
    have hr : ∀ x ∈ ws, 0 ≤ x := fun x hx => hw x (by simp [hx])
    simp only [cov]
    rw [cov_shift a b ws (s + w) hr (by omega), cov_shift s b ws (s + w) hr (by omega)]
    simp only [ov]
    omega

/-- The intervals cover [a, b] exactly once. -/
theorem cov_eq : ∀ (ws : List Int) (s a b : Int), (∀ w ∈ ws, 0 ≤ w) → s ≤ a → a ≤ b →
    b ≤ s + ws.sum → cov a b s ws = b - a
  | [], _, _, _, _, _, _, hb => by simp at hb; simp [cov]; omega
  | w :: ws, s, a, b, hw, hs, hab, hb => by
    have h0 : 0 ≤ w := hw w (by simp)
    have hr : ∀ x ∈ ws, 0 ≤ x := fun x hx => hw x (by simp [hx])
    simp only [List.sum_cons] at hb
    simp only [cov]
    by_cases h1 : b ≤ s + w
    · rw [cov_zero a b ws (s + w) hr h1]; simp only [ov]; omega
    · by_cases h2 : s + w ≤ a
      · rw [cov_eq ws (s + w) a b hr h2 hab (by omega)]; simp only [ov]; omega
      · rw [cov_shift a b ws (s + w) hr (by omega),
            cov_eq ws (s + w) (s + w) b hr (by omega) (by omega) (by omega)]
        simp only [ov]; omega

/-- The sum over j as `settle` computes it is `cov`. -/
theorem range_cov : ∀ (ws : List Int) (s a b : Int),
    ((List.range ws.length).map (fun j => ov a b (s + P ws j) (s + P ws (j + 1)))).sum
      = cov a b s ws
  | [], _, _, _ => by simp [cov]
  | w :: ws, s, a, b => by
    rw [List.length_cons, List.range_succ_eq_map, List.map_cons, List.sum_cons, List.map_map]
    have := range_cov ws (s + w) a b
    simp only [cov, P, List.take_succ_cons, List.take_zero, List.sum_cons, List.sum_nil,
      Function.comp_def] at this ⊢
    rw [← this]
    congr 1
    · congr 1 <;> omega
    · congr 1; apply List.map_congr_left; intro j _
      simp only [Nat.succ_eq_add_one]; congr 1 <;> omega

theorem pos_nonneg (o : List Int) : ∀ w ∈ o.map pos, 0 ≤ w := by
  intro w hw; simp only [List.mem_map] at hw; obtain ⟨x, _, rfl⟩ := hw; simp [pos]; omega
theorem neg_nonneg (o : List Int) : ∀ w ∈ o.map neg, 0 ≤ w := by
  intro w hw; simp only [List.mem_map] at hw; obtain ⟨x, _, rfl⟩ := hw; simp [neg]; omega

theorem P_mono (ws : List Int) (h : ∀ w ∈ ws, 0 ≤ w) : ∀ k, P ws k ≤ P ws (k + 1) := by
  induction ws with
  | nil => intro k; simp [P]
  | cons w ws ih =>
    intro k
    have hr : ∀ x ∈ ws, 0 ≤ x := fun x hx => h x (by simp [hx])
    cases k with
    | zero => simp [P]; exact h w (by simp)
    | succ k => have := ih hr k; simp only [P, List.take_succ_cons, List.sum_cons] at this ⊢; omega

theorem P_le_sum (ws : List Int) (h : ∀ w ∈ ws, 0 ≤ w) : ∀ k, P ws k ≤ ws.sum := by
  induction ws with
  | nil => intro k; simp [P]
  | cons w ws ih =>
    intro k
    have hr : ∀ x ∈ ws, 0 ≤ x := fun x hx => h x (by simp [hx])
    have h0 := h w (by simp)
    cases k with
    | zero => simp [P]; have := ih hr 0; simp [P] at this; omega
    | succ k => have := ih hr k; simp only [P, List.take_succ_cons, List.sum_cons] at this ⊢; omega

theorem P_succ (ws : List Int) : ∀ (i : Nat) (hi : i < ws.length), P ws (i + 1) = P ws i + ws[i] := by
  induction ws with
  | nil => intro i hi; simp at hi
  | cons w ws ih =>
    intro i hi
    cases i with
    | zero => simp [P]
    | succ i =>
      have := ih i (by simp at hi; omega)
      simp only [P, List.take_succ_cons, List.sum_cons, List.getElem_cons_succ] at this ⊢; omega

theorem pos_sub_neg : ∀ o : List Int, (o.map pos).sum - (o.map neg).sum = o.sum
  | [] => by simp
  | x :: o => by have := pos_sub_neg o; simp only [List.map_cons, List.sum_cons, pos, neg] at this ⊢; omega

theorem P_nonneg (ws : List Int) (h : ∀ w ∈ ws, 0 ≤ w) : ∀ k, 0 ≤ P ws k := by
  intro k
  induction k with
  | zero => simp [P]
  | succ k ih => have := P_mono ws h k; omega

theorem ov_comm (a b c d : Int) : ov a b c d = ov c d a b := by simp only [ov]; omega

/-- What a person sends adds up to what they owe. -/
theorem sent_eq (o : List Int) (h : o.sum = 0) (i : Nat) (hi : i < o.length) :
    sent o i = pos o[i] := by
  have hd := P_succ (o.map pos) i (by simp; omega)
  have hlen : (o.map neg).length = o.length := by simp
  have hc := range_cov (o.map neg) 0 (D o i) (D o (i + 1))
  simp only [Int.zero_add, hlen] at hc
  have h1 := P_nonneg _ (pos_nonneg o) i
  have h2 := P_mono _ (pos_nonneg o) i
  have h3 := P_le_sum _ (pos_nonneg o) (i + 1)
  have h4 := pos_sub_neg o
  have e : sent o i = cov (D o i) (D o (i + 1)) 0 (o.map neg) := by
    rw [← hc]; rfl
  rw [e]; simp only [D]
  rw [cov_eq _ 0 _ _ (neg_nonneg o) h1 h2 (by omega)]
  simp at hd; omega

/-- What a person receives adds up to what they are owed. -/
theorem recv_eq (o : List Int) (h : o.sum = 0) (j : Nat) (hj : j < o.length) :
    recv o j = neg o[j] := by
  have hd := P_succ (o.map neg) j (by simp; omega)
  have hlen : (o.map pos).length = o.length := by simp
  have hc := range_cov (o.map pos) 0 (C o j) (C o (j + 1))
  simp only [Int.zero_add, hlen] at hc
  have h1 := P_nonneg _ (neg_nonneg o) j
  have h2 := P_mono _ (neg_nonneg o) j
  have h3 := P_le_sum _ (neg_nonneg o) (j + 1)
  have h4 := pos_sub_neg o
  have e : recv o j = cov (C o j) (C o (j + 1)) 0 (o.map pos) := by
    rw [← hc]; unfold recv t; congr 1; apply List.map_congr_left; intro i _; exact ov_comm _ _ _ _
  rw [e]; simp only [C]
  rw [cov_eq _ 0 _ _ (pos_nonneg o) h1 h2 (by omega)]
  simp at hd; omega

/-- After `settle`, every balance is 0. -/
theorem settle_all_zero (o : List Int) (h : o.sum = 0) (i : Nat) (hi : i < o.length) :
    o[i] - sent o i + recv o i = 0 := by
  rw [sent_eq o h i hi, recv_eq o h i hi]; simp only [pos, neg]; omega
