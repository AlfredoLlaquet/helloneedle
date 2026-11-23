project('clapgrep', 'rust', version: '26.09', meson_version: '>= 1.5')
base_id = 'de.leopoldluley.Clapgrep'
base_path = '/de/leopoldluley/Clapgrep'

i18n = import('i18n')
gnome = import('gnome')

dependency('glib-2.0', version: '>= 2.66')
dependency('gio-2.0', version: '>= 2.66')
dependency('gtk4', version: '>= 4.0.0')
dependency('poppler-glib', version: '>= 24.08.0')

desktop_file_validate = find_program('desktop-file-validate', required: false)
appstreamcli = find_program('appstreamcli', required: false)
cargo = find_program('cargo', required: true)
blueprint_compiler = find_program('blueprint-compiler', version: '>= 0.14.0', required: true)

version = meson.project_version()

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
localedir = prefix / get_option('localedir')

if get_option('profile') == 'development'
  profile = 'Devel'
  vcs_tag = run_command('git', 'rev-parse', '--short', 'HEAD', check: false).stdout().strip()
  if vcs_tag == ''
    version_suffix = '-devel'
  else
    version_suffix = '-@0@'.format(vcs_tag)
  endif
  application_id = '@0@.@1@'.format(base_id, profile)
  application_path = '@0@/@1@'.format(base_path, profile)
else
  profile = ''
  version_suffix = ''
  application_id = base_id
  application_path = base_path
endif

datadir = prefix / get_option('datadir')
pkgdatadir = datadir / meson.project_name()
iconsdir = datadir / 'icons'
podir = meson.project_source_root() / 'locale'
gettext_package = application_id

subdir('locale')
subdir('assets')
subdir('gnome')

# CG4FBS BEGIN ADDITION — Install the license and provenance of the reference application.
if host_machine.system() == 'freebsd'
  reference_docdir = datadir / 'doc' / meson.project_name()
  install_data('LICENSE', 'NOTICE.md', install_dir: reference_docdir)
  install_subdir('LICENSES', install_dir: reference_docdir)
endif
# CG4FBS END

# CG4FBS BEGIN — Refresh desktop icons and file associations after a native FreeBSD install.
# if build_machine.system() == 'linux'
# CG4FBS REPLACEMENT
if build_machine.system() in ['linux', 'freebsd']
# CG4FBS END
  gnome.post_install(
    gtk_update_icon_cache: true,
    update_desktop_database: true,
  )
endif
