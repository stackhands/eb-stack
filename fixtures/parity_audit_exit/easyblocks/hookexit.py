"""Easyblock whose make_module_step exits the interpreter with status 0."""
from easybuild.easyblocks.generic.configuremake import ConfigureMake


class EB_HookExit(ConfigureMake):
    def make_module_step(self, *args, **kwargs):
        raise SystemExit(0)
