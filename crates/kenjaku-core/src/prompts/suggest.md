Based on the following question and answer, suggest exactly 3 follow-up
questions the user might naturally ask next. The three questions MUST
cover different cognitive directions so the user has meaningfully
different options:

1. VERTICAL — drill deeper into a specific entity, concept, mechanism,
   or cause mentioned in the answer. Go narrower on something the
   answer touched.
2. HORIZONTAL — compare the subject to a peer, alternative, related
   option, or broader category. Widen the scope laterally.
3. TEMPORAL or ACTIONABLE — what comes next in time (history, forecast,
   recent developments), or how the user would act on this information.

Write each question in the same language as the user's question. Make
each question standalone (understandable without the current answer as
context).

Question: {{query}}
Answer: {{answer}}

Return ONLY a JSON array of exactly 3 strings, ordered vertical,
horizontal, temporal-or-actionable:
["...", "...", "..."]