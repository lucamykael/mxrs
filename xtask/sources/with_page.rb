Mxrb.define("Minimal.mpr") do
  mendix_version "11.12.1"
  self.module :Sales do
    entity :Order do
      string :Number, documentation: "Stable order number"
      decimal :Total, default: 0
    end

    page(:OrderOverview) do
      title "Orders"
      button(:new_order_button, caption: "New order")
      text(:hint, caption: "Manage your orders")
    end
  end
end
