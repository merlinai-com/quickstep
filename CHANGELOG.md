# Changelog

#### 2025-11-21 18:41 UTC [pending] [main]

##### initialise QuickStep::new and support code

- Core initialisation path:

  * Added `MiniPageBuffer::new` with managed backing storage
  * Introduced `IoEngine::open` for safe file creation
  * Wired up `QuickStep::new` to create the buffer, map table, tree, and IO engine

- Housekeeping:

  * Ignored `quickstep.code-workspace` and removed stray notebook metadata from this changelog

#### 2025-11-21 18:20 UTC [pending] [main]

##### roadmap tasks renumbered + documentation updates

- Roadmap legal-style numbering:

  * Renumbered every phase/task entry to `phase.task`
  * Documented HelixDB testing/integration phases

- Repository documentation touch-ups:

  * README now notes the legal-style numbering scheme
  * Added changelog & coding history scaffolding for future guc runs