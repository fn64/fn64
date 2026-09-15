---
covers: [C3]
---
given a validated campaign dashboard and its receipts
when the unblock-rank report runs
then it lists each frontier cluster with its ROM count and ROM ids, descending by count
and its per-cluster counts sum to the dashboard's frontier counts for that stage
and resource-limit and infrastructure-failure receipts appear in their own rows, never inside a frontier cluster
