---- MODULE SupervisorHold ----
EXTENDS Naturals
VARIABLES now, held, started, atHeld, startedAt
vars == <<now,held,started,atHeld,startedAt>>
Init == /\ now = 0 /\ held = FALSE /\ started = FALSE /\ atHeld = FALSE /\ startedAt = 0
Tick == /\ now < 3 /\ now' = now+1 /\ UNCHANGED <<held,started,atHeld,startedAt>>
Hold == /\ ~held /\ ~started /\ held' = TRUE /\ UNCHANGED <<now,started,atHeld,startedAt>>
AutoRestart == /\ ~started /\ ~held /\ now >= 2
               /\ started' = TRUE /\ atHeld' = held /\ startedAt' = now /\ UNCHANGED <<now,held>>
Next == Tick \/ Hold \/ AutoRestart
NoHeldAutoRestart == ~atHeld
BackoffElapsed == started => startedAt >= 2
====
