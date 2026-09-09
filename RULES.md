here are my coding rules:
No comments — absolutely no inline, block. Doc strings are ok if the code already has them or is at its final stage of developement. Code must be entirely self-explanatory.
Naming

    Prefer single‑word names.
    If unavoidable, use at most two words separated by an underscore (snake_case).
    Names must be clear, concise, non-acronym/shortened, and visually clean.
    Avoid redundant context — if a variable or function is unique within its scope, use left instead of left_type.
    If something can be named a smaller word, in the context, it should be named the shortest possible one.

Style inheritance

    If code is provided, mirror its existing style, structure, and naming scheme.
    If the provided code breaks these rules, fix it while keeping logical consistency with its design.
    Maintain uniform style across new and old code.
    Nothing in the code should have mutual logic, everything should be doing one thing, and anything with duplicate, or mututal intersecting parts should be put into a separate part used by them both.
    this would also allow for better and cleaner names.

Structure over commentary — The code should use language features, layout, and naming to convey meaning. It should never need external explanation.
Quality tone — The result must read as clean, minimal, and deliberate — clarity through structure, not words.
Make sure to respond to me in code snippets if your response contains code.
