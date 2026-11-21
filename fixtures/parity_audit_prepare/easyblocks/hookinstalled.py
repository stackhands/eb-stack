"""Easyblock that needs an installation unless forced to write only a module."""
import os

from easybuild.easyblocks.generic.configuremake import ConfigureMake
from easybuild.tools.build_log import EasyBuildError
from easybuild.tools.config import build_option


class EB_HookInstalled(ConfigureMake):
    def make_module_step(self, *args, **kwargs):
        if not os.path.isdir(os.path.join(self.installdir, 'lib')):
            if not (build_option('force') and build_option('module_only')):
                raise EasyBuildError("No lib directory in %s", self.installdir)
        return super().make_module_step(*args, **kwargs)
