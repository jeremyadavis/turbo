# Turbo Static

Leverages rust-analyzer to build a complete view into the static dependency graph for
your turbo tasks project.

## How it works

- find all occurences of #[turbo_tasks::function] across all the packages you want to query
- for each of the tasks we find, query rust analyzer to see which tasks call them
- apply some very basis control flow analysis to determine whether the call is make 1 time, 0/1 times, or 0+ times,
  corresponding to direct calls, conditionals, or for loops. nested conditionals collapse

## Stretch goals

- evaluate where Vcs end up to track data flow through the app also
- a few different visualization formats
  - dot
  - neoj4 / graph dbs
