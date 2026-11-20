Require an empty `versionsuffix` in `build_order::satisfies` when a
dependency omits the versionsuffix tuple element, matching EasyBuild's
empty-suffix default while still allowing exact combined stem+suffix pins.
