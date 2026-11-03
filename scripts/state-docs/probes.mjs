const cases = [
  [
    "ports",
    "reuse-occupied-port",
    "UniquePorts",
    "/\\ p \\notin {rows[x] : x \\in Owners}",
    "/\\ TRUE",
  ],
  [
    "ports",
    "stale-snapshot-save",
    "RetainCommitted",
    "Next == \\E o \\in Owners, p \\in {1,2} : Allocate(o,p)",
    "Next == (\\E o \\in Owners, p \\in {1,2} : Allocate(o,p)) \\/ (/\\ rows[1] # 0 /\\ rows[2] # 0 /\\ rows' = [rows EXCEPT ![2] = 0] /\\ UNCHANGED committed)",
  ],
  [
    "identity",
    "ignore-known-mismatch",
    "KnownIdentityMatches",
    "/\\ (recorded = 0 \\/ recorded = current)",
    "/\\ TRUE",
  ],
  [
    "identity",
    "signal-zero-target",
    "PositiveTarget",
    "/\\ pid > 0",
    "/\\ TRUE",
  ],
  [
    "supervision",
    "restart-held-service",
    "NoHeldAutoRestart",
    "/\\ ~started /\\ ~held /\\ now",
    "/\\ ~started /\\ now",
  ],
  [
    "supervision",
    "restart-before-deadline",
    "BackoffElapsed",
    "/\\ now >= 2",
    "/\\ TRUE",
  ],
  [
    "setup",
    "skip-first-stage",
    "FirstSettledBeforeSecond",
    "/\\ settled /\\ ~failed",
    "/\\ ~failed",
  ],
  [
    "setup",
    "continue-after-failed-stage",
    "FailureStopsSequence",
    "/\\ settled /\\ ~failed",
    "/\\ settled",
  ],
];
export const probes = cases.map(([document, name, rule, before, after]) => ({
  document,
  name,
  expectedRules: [rule],
  mutate(spec) {
    if (spec.split(before).length !== 2)
      throw new Error("Mutation target must match once");
    return spec.replace(before, after);
  },
}));
