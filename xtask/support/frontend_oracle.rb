# frozen_string_literal: true
require_relative 'command_oracle_support'
require 'zip'

# `frontend migrate` previews, and under --apply writes, the frontend model
# migrations: pluggable widgets rebound to the schema their installed .mpk
# declares, legacy layout-grid weights normalized, and renamed design
# properties carried to their theme's new names. Both CLIs must print the
# same preview text and JSON, refuse the same issues, and exit alike; each
# applies a safe plan to its own copy of a model, and afterwards both find
# nothing left to migrate in either copy — what mxrb wrote and what mxrs
# wrote, which must also be the same document apart from fresh identities.
#
# Normalized before comparing: each CLI's name in its prefix and usage
# ("[mxrb]", "mxrb frontend"), and the cause a package that is not a zip is
# reported with after "cannot read widgets/<name>.mpk: " (rubyzip's exception
# class and text in mxrb, the zip crate's in mxrs).
o = CommandOracle.new('frontend')

def normalize(value)
  return value unless value.is_a?(String)

  value.gsub('[mxrb]', '[mxrs]')
       .gsub('mxrb frontend', 'mxrs frontend')
       .gsub(%r{(cannot read widgets/[^:"\n]+): [^"\n]*}, '\1: <cause>')
end

def same(o, *args)
  native = o.run(true, *args).map { normalize(_1) }
  ours = o.run(false, *args).map { normalize(_1) }
  o.check("frontend mismatch for #{args.inspect}: #{native.inspect} != #{ours.inspect}") { native == ours }
  ours
end

def items(value) = Mxrb::IO::BsonCodec.parse_array(value)[:items]
def array(values, marker = 2) = Mxrb::IO::BsonCodec.build_array(values, marker:)

def property_xml(key, type, default = nil, children = nil)
  default_attribute = default ? %( defaultValue="#{default}") : ''
  return %(<property key="#{key}" type="#{type}"#{default_attribute}><caption>#{key}</caption></property>) unless children

  nested = children.map { property_xml(*_1) }.join
  %(<property key="#{key}" type="#{type}" isList="true"><caption>#{key}</caption>) +
    %(<properties><propertyGroup caption="Item">#{nested}</propertyGroup></properties></property>)
end

def widget_xml(id, properties)
  %(<widget id="#{id}" supportedPlatform="Web" pluginWidget="true"><name>#{id}</name>) +
    %(<properties><propertyGroup caption="General">#{properties.map { property_xml(*_1) }.join}) +
    %(</propertyGroup></properties></widget>)
end

def package(path, *xmls)
  FileUtils.mkdir_p(File.dirname(path))
  Zip::File.open(path, create: true) do |zip|
    xmls.each_with_index { |xml, index| zip.get_output_stream("Widget#{index}.xml") { _1.write(xml) } }
  end
end

# A widget as the package `properties` describe stored it.
def stored_widget(scratch, id, properties)
  path = File.join(scratch, "#{SecureRandom.hex(6)}.mpk")
  package(path, widget_xml(id, properties))
  type, object = Mxrb::WidgetPackage.template(Mxrb::WidgetPackage.new(path).definition(id))
  { '$ID' => SecureRandom.uuid, '$Type' => 'CustomWidgets$CustomWidget', 'Name' => id.split('.').last,
    'Type' => type, 'Object' => object }
end

def property_of(widget, key)
  type = items(widget.dig('Type', 'ObjectType', 'PropertyTypes')).find { _1['PropertyKey'] == key }
  [type, items(widget.dig('Object', 'Properties')).find { _1['TypePointer'] == type['$ID'] }]
end

def project(dir, version)
  path = File.join(dir, 'App.mpr')
  FileUtils.mkdir_p(dir)
  Mxrb.define(path) do
    mendix_version version
    self.module(:App) { entity :Item }
  end
  path
end

def insert(path, document, compatibility: true)
  mpr = Mxrb::IO::MprFile.open(path, apply_studio_compatibility: compatibility)
  mpr.insert_unit(container_uuid: mpr.root_unit.fetch('UnitID'), containment_name: 'Documents',
                  contents_doc: { '$ID' => SecureRandom.uuid, '$Type' => 'Forms$Page' }.merge(document))
ensure
  mpr&.close
end

def row(*weights)
  columns = weights.map do |weight|
    { '$ID' => SecureRandom.uuid, '$Type' => 'Forms$LayoutGridColumn', 'Weight' => weight,
      'TabletWeight' => -1, 'PhoneWeight' => -1 }
  end
  { '$ID' => SecureRandom.uuid, '$Type' => 'Forms$LayoutGridRow', 'Columns' => array(columns) }
end

def design_property(key, option)
  { '$ID' => SecureRandom.uuid, '$Type' => 'Forms$DesignPropertyValue', 'Key' => key,
    'Value' => { '$ID' => SecureRandom.uuid, '$Type' => 'Forms$OptionDesignPropertyValue', 'Option' => option } }
end

def compound_property(key, *properties)
  { '$ID' => SecureRandom.uuid, '$Type' => 'Forms$DesignPropertyValue', 'Key' => key,
    'Value' => { '$ID' => SecureRandom.uuid, '$Type' => 'Forms$CompoundDesignPropertyValue',
                 'Properties' => array(properties) } }
end

def theme(dir)
  web = File.join(dir, 'themesource', 'atlas_core', 'web')
  FileUtils.mkdir_p(web)
  File.write(File.join(web, 'design-properties.json'), JSON.pretty_generate(
    'Widget' => [
      { 'name' => 'Spacing', 'type' => 'Spacing',
        'margin' => [{ 'name' => 'M', 'bottom' => { 'oldNames' => ['Spacing bottom::Outer medium'] },
                       'top' => { 'oldNames' => ['Old top::M'] } }] },
      { 'name' => 'Card style', 'type' => 'Dropdown', 'oldNames' => ['Card'],
        'options' => [{ 'name' => 'Raised', 'oldNames' => ['Lifted'] }] }
    ]
  ))
  native = File.join(dir, 'themesource', 'atlas_core', 'native')
  FileUtils.mkdir_p(native)
  File.write(File.join(native, 'design-properties.json'), '{broken')
end

ITEM = [%w[name string]].freeze
NEW_EXAMPLE = [
  %w[mode enumeration cards], %w[debounce integer 300], %w[required expression false],
  ['source', 'datasource'], ['items', 'object', nil, [*ITEM, %w[added integer 7]]]
].freeze
OLD_EXAMPLE = [
  %w[mode enumeration list], %w[required boolean false], ['source', 'datasource'],
  ['items', 'object', nil, ITEM]
].freeze
CURRENT = [%w[label string]].freeze

# Every migration the plan applies: a widget rebound (a configured
# enumeration kept, a stale field dropped, a boolean become an expression, a
# data source normalized, a nested object rebound onto its grown schema), a
# widget already current, layout rows with and without an array marker,
# legacy spacing folded and a design property renamed.
def safe_fixture(dir, scratch)
  path = project(dir, '11.12.1')
  package(File.join(dir, 'widgets', 'example.mpk'), widget_xml('example.Widget', NEW_EXAMPLE))
  package(File.join(dir, 'widgets', 'current.mpk'), widget_xml('example.Current', CURRENT))
  theme(dir)
  widget = stored_widget(scratch, 'example.Widget', OLD_EXAMPLE)
  property_of(widget, 'mode').last['Value']['TextTemplate'] = { 'stale' => true }
  property_of(widget, 'required').last['Value']['PrimitiveValue'] = 'true'
  property_of(widget, 'source').last['Value']['DataSource'] = {
    '$ID' => SecureRandom.uuid, '$Type' => 'Forms$MicroflowSettings', 'Microflow' => 'App.DS_Items'
  }
  items_type, items_property = property_of(widget, 'items')
  nested = Mxrb::Frontend::Migrator.allocate.send(:object_template, items_type.dig('ValueType', 'ObjectType'))
  items(nested['Properties']).first['Value']['PrimitiveValue'] = 'kept'
  items_property['Value']['Objects'] = array([nested])
  insert(path, {
    'Name' => 'Home',
    'Widgets' => array([widget, stored_widget(scratch, 'example.Current', CURRENT)], 3),
    'Appearance' => { '$Type' => 'Forms$Appearance', 'DesignProperties' => array([
      design_property('Spacing bottom', 'Outer medium'), design_property('Card', 'Lifted'),
      design_property('Plain', 'On')
    ]) },
    'Rows' => [row(-2, -1, -2), row(3, -1, -2), row(4, 8)],
    'Row' => { '$Type' => 'Forms$LayoutGridRow',
               'Columns' => [{ '$Type' => 'Forms$LayoutGridColumn', 'Weight' => -1 }] }
  }, compatibility: false)
  path
end

# Every issue a plan can raise on a supported model, each on its own widget,
# row or design property, beside an unreadable package.
def blocked_fixture(dir, scratch)
  path = project(dir, '11.12.1')
  removed = [%w[legacy string]]
  changed = [%w[count string]]
  nested = [['items', 'object', nil, ITEM]]
  package(File.join(dir, 'widgets', 'example.mpk'),
          widget_xml('example.Removed', [%w[replacement string]]),
          widget_xml('example.Changed', [%w[count integer]]),
          widget_xml('example.Current', CURRENT), widget_xml('example.Nested', nested))
  package(File.join(dir, 'widgets', 'dup1.mpk'), widget_xml('example.Dup', CURRENT))
  package(File.join(dir, 'widgets', 'dup2.mpk'), widget_xml('example.Dup', CURRENT))
  File.binwrite(File.join(dir, 'widgets', 'broken.mpk'), 'not a zip')
  theme(dir)

  widgets = []
  widgets << stored_widget(scratch, 'example.Removed', removed).tap do
    property_of(_1, 'legacy').last['Value']['PrimitiveValue'] = 'important'
  end
  widgets << stored_widget(scratch, 'example.Changed', changed)
  widgets << stored_widget(scratch, 'example.Missing', CURRENT)
  widgets << stored_widget(scratch, 'example.Dup', CURRENT)
  widgets << stored_widget(scratch, 'example.Current', CURRENT).tap { _1.delete('Object') }
  widgets << stored_widget(scratch, 'example.Current', CURRENT).tap { _1['Type']['Mystery'] = true }
  widgets << stored_widget(scratch, 'example.Current', CURRENT).tap { _1['Object']['Mystery'] = 1 }
  widgets << stored_widget(scratch, 'example.Current', CURRENT).tap do
    items(_1.dig('Object', 'Properties')).first['TypePointer'] = SecureRandom.uuid
  end
  widgets << stored_widget(scratch, 'example.Current', CURRENT).tap do
    items(_1.dig('Object', 'Properties')).first['Value'] = 'oops'
  end
  widgets << stored_widget(scratch, 'example.Nested', nested).tap do |widget|
    type, property = property_of(widget, 'items')
    object = Mxrb::Frontend::Migrator.allocate.send(:object_template, type.dig('ValueType', 'ObjectType'))
    property['Value']['Objects'] = array([object])
    type['ValueType']['ObjectType'] = nil
  end
  insert(path, {
    'Name' => 'Blocked', 'Widgets' => array(widgets, 3),
    'Rows' => [row(0, -1), row(13, -1), row('x', -1), row(1, 10)],
    'Appearance' => { '$Type' => 'Forms$Appearance', 'DesignProperties' => array([
      compound_property('Spacing', design_property('margin-top', 'L')), design_property('Old top', 'M')
    ]) }
  })
  path
end

def strip_identities(value)
  case value
  when Hash then value.reject { |key, _| %w[$ID TypePointer].include?(key) }.map { [_1, strip_identities(_2)] }
  when Array then value.map { strip_identities(_1) }
  when BSON::Binary then :identity
  else value
  end
end

def page_document(path, name)
  mpr = Mxrb::IO::MprFile.open(path, readonly: true)
  unit = mpr.all_units.find { mpr.parse_contents(_1)['Name'] == name }
  strip_identities(mpr.parse_contents(unit))
ensure
  mpr&.close
end

Dir.mktmpdir('mxrs-frontend-oracle-') do |root|
  scratch = File.join(root, 'scratch')
  FileUtils.mkdir_p(scratch)

  safe = safe_fixture(File.join(root, 'safe'), scratch)
  o.readonly(File.dirname(safe)) do
    out, = same(o, 'migrate', safe)
    o.check("safe preview counts: #{out}") do
      out.include?("Changes           : 1\n") && out.include?("Widgets           : 1\n") &&
        out.include?("Layout rows       : 3\n") && out.include?("Design properties : 2\n") &&
        out.include?("Safe              : true\n")
    end
    same(o, 'migrate', safe, '--json')
  end

  blocked = blocked_fixture(File.join(root, 'blocked'), scratch)
  o.readonly(File.dirname(blocked)) do
    out, _, code = same(o, 'migrate', blocked)
    expected = %w[INVALID_WIDGET_PACKAGE REMOVED_CONFIGURED_WIDGET_PROPERTY CHANGED_WIDGET_PROPERTY
                  MISSING_WIDGET_DEFINITION AMBIGUOUS_WIDGET_DEFINITION MALFORMED_WIDGET UNKNOWN_WIDGET_SCHEMA
                  UNKNOWN_WIDGET_OBJECT UNKNOWN_PROPERTY_POINTER MALFORMED_WIDGET_VALUE CHANGED_WIDGET_OBJECT
                  UNSAFE_LAYOUT_WEIGHTS CONFLICTING_DESIGN_PROPERTY]
    found = out.scan(/^\[([A-Z_]+)\]/).flatten.uniq
    o.check("blocked issue kinds: #{found.inspect}") { code == 1 && expected.all? { found.include?(_1) } }
    same(o, 'migrate', blocked, '--json')
    same(o, 'migrate', blocked, '--apply')
    same(o, 'migrate', blocked, '--apply', '--json')
  end

  unsupported = project(File.join(root, 'unsupported'), '9.24.0')
  o.readonly(File.dirname(unsupported)) do
    same(o, 'migrate', unsupported, '--json')
    out, _, code = same(o, 'migrate', unsupported, '--apply')
    o.check('unsupported version') { code == 1 && out.include?('[UNSUPPORTED_VERSION] : Mendix "9.24.0"') }
  end

  legacy = project(File.join(root, 'legacy'), '10.24.0.73019')
  insert(legacy, { 'Name' => 'Grid', 'Rows' => [row(-1), row(-2, -1, -2), row(3, -1, -2)] })
  o.readonly(File.dirname(legacy)) { same(o, 'migrate', legacy) }

  [[safe, '--json'], [legacy, nil]].each do |source, flag|
    native_copy = File.join(root, "native-#{File.basename(File.dirname(source))}")
    ours_copy = File.join(root, "ours-#{File.basename(File.dirname(source))}")
    FileUtils.cp_r(File.dirname(source), native_copy)
    FileUtils.cp_r(File.dirname(source), ours_copy)
    native_path = File.join(native_copy, 'App.mpr')
    ours_path = File.join(ours_copy, 'App.mpr')
    flags = ['--apply', flag].compact
    native = o.run(true, 'migrate', native_path, *flags).map { normalize(_1) }
    ours = o.run(false, 'migrate', ours_path, *flags).map { normalize(_1) }
    o.check("apply mismatch: #{native.inspect} != #{ours.inspect}") { native == ours && ours.last.zero? }
    [native_path, ours_path].each do |path|
      out, = same(o, 'migrate', path)
      o.check("#{path} still migrates after apply: #{out}") { out.include?("Changes           : 0\n") }
      same(o, 'migrate', path, '--json')
    end
    name = source == safe ? 'Home' : 'Grid'
    lhs = page_document(native_path, name)
    rhs = page_document(ours_path, name)
    o.check("applied #{name} documents differ:\n#{lhs.inspect}\n#{rhs.inspect}") { lhs == rhs }
  end

  o.help
  same(o)
  same(o, 'help')
  same(o, 'migrate', '--help')
  same(o, 'unknown')
  same(o, 'migrate')
  same(o, 'migrate', safe, 'extra')
  o.failure('migrate', safe, '--unknown')
  o.failure('migrate', File.join(root, 'Absent.mpr'))
end
o.finish
