"""Easyblock whose make_module_dep loads a module no dependency names."""
from easybuild.easyblocks.generic.configuremake import ConfigureMake


class EB_HookLoad(ConfigureMake):
    def make_module_dep(self, *args, **kwargs):
        loads = super().make_module_dep(*args, **kwargs)
        return loads + self.module_generator.load_module('hookdep/1.0')
