---- MODULE ProcessAdmission ----
EXTENDS Integers
VARIABLES recorded, current, pid, accepted, atKnown, atCurrent, atPid
vars == <<recorded,current,pid,accepted,atKnown,atCurrent,atPid>>
Init == /\ recorded \in {0,1,2} /\ current \in {1,2} /\ pid \in {0,1}
        /\ accepted = FALSE /\ atKnown = 0 /\ atCurrent = 0 /\ atPid = 0
Admit == /\ ~accepted /\ pid > 0 /\ (recorded = 0 \/ recorded = current)
         /\ accepted' = TRUE /\ atKnown' = recorded /\ atCurrent' = current /\ atPid' = pid
         /\ UNCHANGED <<recorded,current,pid>>
Next == Admit
KnownIdentityMatches == (accepted /\ atKnown # 0) => atKnown = atCurrent
PositiveTarget == accepted => atPid > 0
====
