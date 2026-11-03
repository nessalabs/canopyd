---- MODULE PortTransaction ----
EXTENDS Naturals, FiniteSets
VARIABLES rows, committed
vars == <<rows,committed>>
Owners == {1,2}
Init == /\ rows = [o \in Owners |-> 0] /\ committed = [o \in Owners |-> 0]
Allocate(o,p) == /\ rows[o] = 0 /\ p \notin {rows[x] : x \in Owners}
                 /\ rows' = [rows EXCEPT ![o] = p] /\ committed' = [committed EXCEPT ![o] = p]
Next == \E o \in Owners, p \in {1,2} : Allocate(o,p)
UniquePorts == rows[1] = 0 \/ rows[2] = 0 \/ rows[1] # rows[2]
RetainCommitted == \A o \in Owners : committed[o] # 0 => rows[o] = committed[o]
====
