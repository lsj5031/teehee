---------------- MODULE SenderPacing ----------------
\* Abstract model of teehee's sender pacing arithmetic.
\*
\* Each packet carries ChunkFrames = floor(SampleRate * ChunkMs /
\* 1000) frames and is paced once per ChunkMs.  Over a 3-second
\* macro-step exactly 3000/ChunkMs packets are sent, while the
\* receiver consumes SampleRate * 3 frames.  The residual deficit is
\* what the sender's drift compensator must remove.
\*
\* MaxHelpPerStep is the largest possible increase in sent frames
\* from the +MaxPpm clamp.  BufferNeverNegative checks that a buffer
\* of BufferMs can survive; PpmWithinClamp checks whether the clamp
\* is even large enough to express the needed rate.

EXTENDS Naturals

CONSTANTS SampleRate, ChunkMs, BufferMs, MaxPpm

StepMs == 3000

ChunkFrames == (SampleRate * ChunkMs) \div 1000
BufferFrames == (SampleRate * BufferMs) \div 1000

PacketsPerStep == StepMs \div ChunkMs
FramesSentPerStep == PacketsPerStep * ChunkFrames
FramesConsumedPerStep == SampleRate * (StepMs \div 1000)
DeficitPerStep == FramesConsumedPerStep - FramesSentPerStep

MaxHelpPerStep == (FramesSentPerStep * MaxPpm) \div 1000000
ResidualDeficit == DeficitPerStep - MaxHelpPerStep

\* Required period shortening in parts per million:
\* (ChunkMs * SampleRate) / (1000 * ChunkFrames) - 1.
RequiredPpm ==
    ((ChunkMs * SampleRate * 1000) \div ChunkFrames) - 1000000

VARIABLES buffer, steps

Init == buffer = BufferFrames /\ steps = 0

Next ==
    \/ ( /\ buffer > 0
          /\ ResidualDeficit > 0
          /\ buffer' = buffer - ResidualDeficit
          /\ steps' = steps + 1 )
    \/ UNCHANGED <<buffer, steps>>

Spec == Init /\ [][Next]_<<buffer, steps>>

BufferNeverNegative == buffer >= 0

PpmWithinClamp == RequiredPpm <= MaxPpm
PpmExceedsClamp == RequiredPpm > MaxPpm

=============================================================================
