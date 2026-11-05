---------------- MODULE PrebufferAnchor ----------------
\* Abstract model of teehee's prebuffer phase.
\*
\* While head = Nil, push() stores packets and tracks the smallest
\* sequence seen in minPushed.  pop_frames() waits for Target filled
\* slots and then anchors at minPushed.  With startup packet loss,
\* more than Cap sequence numbers can be needed to accumulate Target
\* packets, so the slot holding minPushed can be overwritten before
\* the gate releases.
\*
\* AnchorSound is the property the anchor is supposed to provide:
\* the slot selected by minPushed still holds minPushed.
\* NoRetainedDiscard says playout never throws away a packet that
\* was still buffered and ahead of the anchor.

EXTENDS Naturals, FiniteSets

CONSTANTS SeqMax, Cap, Target

ASSUME SeqMax \in Nat /\ SeqMax > 1
ASSUME Cap \in Nat /\ Cap > 0
ASSUME Target \in Nat /\ Target > 0 /\ Target <= Cap

Seq == 0 .. SeqMax - 1
\* Sentinel outside the sequence space.
Nil == SeqMax

Idx(s) == s % Cap

VARIABLES head, slots, minPushed, dropped, overwritten

Init ==
    /\ head = Nil
    /\ slots = [i \in 0 .. Cap - 1 |-> Nil]
    /\ minPushed = Nil
    /\ dropped = {}
    /\ overwritten = {}

EarlierOrEqual(a, b) == (b - a) % SeqMax < SeqMax \div 2

FilledCount == Cardinality({i \in 0 .. Cap - 1 : slots[i] /= Nil})

Push(s) ==
    /\ head = Nil
    /\ s \in Seq
    /\ LET i == Idx(s)
           old == slots[i]
       IN /\ slots' = [slots EXCEPT ![i] = s]
          /\ minPushed' = IF minPushed = Nil \/ EarlierOrEqual(s, minPushed)
                          THEN s ELSE minPushed
          /\ overwritten' = IF old /= Nil /\ old /= s
                            THEN overwritten \cup {old} ELSE overwritten
          /\ UNCHANGED <<head, dropped>>

\* The gate has released.  Anchor at minPushed and play one whole
\* packet.  A mismatched slot produces silence and is cleared anyway,
\* exactly as pop_frames() does at a packet boundary.
Pop ==
    /\ head = Nil
    /\ FilledCount >= Target
    /\ LET h == minPushed
           i == Idx(h)
           present == slots[i]
       IN /\ head' = (h + 1) % SeqMax
          /\ slots' = [slots EXCEPT ![i] = Nil]
          /\ dropped' = IF present /= Nil /\ present /= h
                        THEN dropped \cup {present} ELSE dropped
          /\ UNCHANGED <<minPushed, overwritten>>

Next == (\E s \in Seq : Push(s)) \/ Pop

Spec == Init /\ [][Next]_<<head, slots, minPushed, dropped, overwritten>>

AnchorSound ==
    minPushed = Nil \/ slots[Idx(minPushed)] = minPushed

NoRetainedDiscard == dropped = {}

=============================================================================
