import { invokeAction } from '@/api/actions';
import type { Widget, WidgetContext } from '@/types/model';
import { text, widgetCaption, widgetKey, widgetOptions, widgetStyle } from '@/utils/widgets';

import { BoundInput } from './BoundInput';

type WidgetViewProps = { widget: Widget; context: WidgetContext };

/** One widget of the model, and everything nested in it. */
export function WidgetView({ widget, context }: WidgetViewProps) {
  const className = text(widgetOptions(widget).class) || undefined;
  const content = (widget.children || []).map((child, index) => (
    <WidgetView key={widgetKey(child, index)} widget={child} context={context} />
  ));

  switch (widget.type) {
    case 'container':
      return (
        <div className={className} style={widgetStyle(widget)}>
          {content}
        </div>
      );
    case 'text':
      return <span className={className}>{widgetCaption(widget)}</span>;
    case 'button':
      return (
        <button
          className={className}
          type="button"
          onClick={() => invokeAction(widget.events?.[0], context).catch(console.error)}
        >
          {widgetCaption(widget)}
        </button>
      );
    case 'text_box':
    case 'check_box':
    case 'date_picker':
    case 'drop_down':
    case 'combo_box':
      return <BoundInput widget={widget} context={context} />;
    case 'data_view':
      return (
        <section className={className} data-widget="data-view">
          {content}
        </section>
      );
    case 'layout_grid':
      return <div className="mxrs-layout-grid">{content}</div>;
    case 'layout_grid_row':
      return <div className="mxrs-layout-row">{content}</div>;
    case 'layout_grid_column': {
      const weight = Number(widgetOptions(widget).desktop || 1);
      return (
        <div className="mxrs-layout-column" style={{ flexGrow: weight }}>
          {content}
        </div>
      );
    }
    case 'data_grid':
    case 'data_grid_2':
      return (
        <section className="mxrs-data-grid">
          <strong>{widgetCaption(widget)}</strong>
          <table>
            <tbody>
              <tr>
                <td>No runtime records loaded</td>
              </tr>
            </tbody>
          </table>
        </section>
      );
    case 'gallery':
      return (
        <section className="mxrs-gallery">
          {content.length ? content : 'No runtime records loaded'}
        </section>
      );
    case 'pluggable':
      return (
        <section className={className} data-widget-id={text(widgetOptions(widget).widget_id)}>
          {content.length ? content : widgetCaption(widget)}
        </section>
      );
    default:
      return (
        <section className="mxrs-unsupported" data-widget-type={widget.type}>
          {widgetCaption(widget)}
          {content}
        </section>
      );
  }
}
