### A shot list's `notes` can be one string (#1702)
**Package.** engine
**What changed.** `notes = "the whole tour"` in a `[[shot]]` of a `flint capture` shot list was refused ("expected a sequence") although the book called `notes` free text; a string now reads as one line, the same as `notes = ["the whole tour"]`. Lists read as before, and anything else is refused with "notes must be a string or a list of strings".
**What to do.** Nothing.
