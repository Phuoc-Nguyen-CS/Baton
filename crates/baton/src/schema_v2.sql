-- Schema v2: a one-line description of each decision for the owner.
ALTER TABLE decision ADD COLUMN summary TEXT NOT NULL DEFAULT '';  -- e.g. "Bash: npm install"
