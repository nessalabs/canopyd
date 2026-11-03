---- MODULE SetupOrder ----
EXTENDS Naturals
VARIABLES settled, failed, started, atSettled, atFailed
vars == <<settled,failed,started,atSettled,atFailed>>
Init == /\ settled = FALSE /\ failed = FALSE /\ started = FALSE /\ atSettled = FALSE /\ atFailed = FALSE
Finish(result) == /\ ~settled /\ settled' = TRUE /\ failed' = result /\ UNCHANGED <<started,atSettled,atFailed>>
StartSecond == /\ ~started /\ settled /\ ~failed
               /\ started' = TRUE /\ atSettled' = settled /\ atFailed' = failed /\ UNCHANGED <<settled,failed>>
Next == (\E result \in BOOLEAN : Finish(result)) \/ StartSecond
FirstSettledBeforeSecond == started => atSettled
FailureStopsSequence == ~atFailed
====
