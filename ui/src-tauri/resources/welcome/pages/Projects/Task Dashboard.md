This table is a live read-only query of the graph's index. It starts with three open tasks from [[Projects/Observatory Night/Plan]].

{{query SELECT p.title AS page, t.state AS state, b.content AS task, t.scheduled_date AS scheduled, t.deadline_date AS deadline FROM tasks t JOIN blocks b ON b.id = t.block_id JOIN pages p ON p.id = b.page_id WHERE t.state IN ('TODO', 'DOING') AND p.title = 'Projects/Observatory Night/Plan' AND p.is_journal = 0 ORDER BY t.state, b.content}}

Mark one of those tasks **DONE**, save the edit, and return here. The dashboard reads the current indexed task states rather than a copied list. Empty date cells mean no date is assigned.

## Make the query yours

Click the query block to inspect its source. `tasks.block_id` joins to `blocks.id`, and `blocks.page_id` joins to `pages.id`. The `tasks` table holds `state`, `scheduled_date`, and `deadline_date`; the page title and journal flag come from `pages`.

Remove the page-title condition to see open tasks across your graph. Keep the query a `SELECT`: inline queries are for reading, not modifying the database.

Use SQL `ORDER BY` to choose the order of query results. An ordinary hand-written table works differently: its clickable headings sort and save the Markdown rows; compare [[Writing/Tables]]. See [[Projects/Tasks And Dates]] to populate the date columns.
