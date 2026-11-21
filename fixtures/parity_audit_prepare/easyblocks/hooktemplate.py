"""Easyblock whose module names a path through the %(installdir)s template."""
from easybuild.easyblocks.generic.configuremake import ConfigureMake


class EB_HookTemplate(ConfigureMake):
    def make_module_step(self, *args, **kwargs):
        self.cfg['modextravars'] = {'HOOKTEMPLATE_DATA': '%(installdir)s/data'}
        return super().make_module_step(*args, **kwargs)
