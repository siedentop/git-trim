# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Types of Change

-   `Added` for new features.
-   `Changed` for changes in existing functionality.
-   `Deprecated` for soon-to-be removed features.
-   `Removed` for now removed features.
-   `Fixed` for any bug fixes.
-   `Security` in case of vulnerabilities.

## [Unreleased]

### Added

-   `--porcelain` option to provide output for scripting without deleting any
    branches. Options are `local`, `remote` or `json`. Those list local branches
    that should be deleted, remote branches or all output in structured JSON.
    The JSON can be further filtered with _jq_ or _gron_.

### Changed

-   Performance increase for big repos. Associating each local branch with all
    remotes is now multiple orders of magnitude faster. There are still
    bottlenecks that make the use on big repos impractically slow.
