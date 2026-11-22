### Entities sharing a skinned model draw their first frame the same every run (#1177)
**What changed.** Several entities on one skinned asset (a bartender and two patrons on one `.glb`) share a fallback palette that an entity draws with until it writes its own bones. It was whichever entity wrote last, which wasn't the same one every run, so a stepped capture or screenshot of that frame could differ between two runs. It is now always the first entity by name's pose; the style scissor's bounds fall back to it too.
**What to do.** Nothing.
