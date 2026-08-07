-- V7: add optional subcategory column for finer-grained memory labels.
--
-- A subcategory is an open-ended sub-label under a category
-- (`preference.coding / testing`, `fact.person / family`) that recall
-- and consolidation can scope to. Nullable: most memories have none,
-- and degraded/verbatim mode never sets one.

ALTER TABLE memories ADD COLUMN subcategory TEXT;
