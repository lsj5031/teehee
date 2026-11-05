---------------- MODULE JitterRing ----------------
\* Abstract model of teehee's receive ring indexing.
\*
\* The ring stores a packet with sequence s in slot (s % Cap).  The
\* real code uses u32 sequence numbers, so this model uses a small
\* finite sequence space SeqMax and wraps at SeqMax.  The initial
\* state is the legitimate steady state "play head is the last
\* sequence before wrap, and its packet is buffered".
\*
\* Invariant AnchorConsistent says: if a play head exists, the slot
\* selected for it is either empty or holds that exact sequence.
\* A push of sequence 0 must preserve that property for the ring to
\* be lossless across wrap.

EXTENDS Naturals

CONSTANTS SeqMax, Cap

ASSUME SeqMax \in Nat /\ SeqMax > 1
ASSUME Cap \in Nat /\ Cap > 0

Seq == 0 .. SeqMax - 1
\* Sentinel outside the sequence space.  TLA+ parses a bare negative
\* literal poorly in this position, so use a positive out-of-range value.
Nil == SeqMax
Half == SeqMax \div 2

Idx(s) == s % Cap

VARIABLES head, slots

Init ==
    /\ head = SeqMax - 1
    /\ slots = [i \in 0 .. Cap - 1 |->
                   IF i = Idx(SeqMax - 1) THEN SeqMax - 1 ELSE Nil]

IsAtOrBefore(s, h) == (h - s) % SeqMax < Half
Ahead(s, h) == (s - h) % SeqMax

\* Mirrors push()'s ordering checks with head_offset = 0.  A seq that
\* is at least Cap ahead would trigger a stream reset in the real
\* code, so it is not part of this collision model.
Push(s) ==
    /\ s \in Seq
    /\ head /= Nil
    /\ LET h == head
           fwd == (h - s) % SeqMax
           late == IsAtOrBefore(s, h) /\ (fwd > 0)
           ahead == Ahead(s, h)
       IN ~late /\ ahead < Cap
    /\ slots' = [slots EXCEPT ![Idx(s)] = s]
    /\ UNCHANGED head

\* Consume one whole packet at the head and advance.
Pop ==
    /\ head /= Nil
    /\ slots[Idx(head)] = head
    /\ slots' = [slots EXCEPT ![Idx(head)] = Nil]
    /\ head' = (head + 1) % SeqMax

Next == (\E s \in Seq : Push(s)) \/ Pop

Spec == Init /\ [][Next]_<<head, slots>>

AnchorConsistent ==
    head = Nil
    \/ slots[Idx(head)] = Nil
    \/ slots[Idx(head)] = head

=============================================================================
