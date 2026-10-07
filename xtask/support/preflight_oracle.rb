# frozen_string_literal: true
require_relative 'command_oracle_support'

# `preflight` says what a model holds that the native compilers and runtime
# do not take. Which widgets and activities those are is each tool's own
# coverage, so only a model both take whole is compared line for line: the
# header, the summary, and mxrb's JSON fields (mxrs's report adds stats).
o = CommandOracle.new('preflight')

def define(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Sales) do
      entity(:Order) { string :Number }
      microflow(:ACT_Order_Save) {}
    end
  end
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  mpr.ensure_storage_for_version!('11.12.1')
  mpr.close
end

Dir.mktmpdir('mxrs-preflight-oracle-') do |root|
  path = File.join(root, 'Project.mpr')
  define(path)
  o.readonly(root) do
    o.equivalent(path) { |native, ours| [native.sub('[mxrb]', '[mxrs]'), ours] }
    native = JSON.parse(o.success(true, path, '--json'))
    ours = JSON.parse(o.success(false, path, '--json'))
    ours['stats'] = ours['stats'].slice(*native['stats'].keys)
    o.check("JSON mismatch: #{native.inspect} != #{ours.inspect}") { native == ours }
  end
  o.failure
  o.failure(File.join(root, 'Absent.mpr'))
end
o.finish
