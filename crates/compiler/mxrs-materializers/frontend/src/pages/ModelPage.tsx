import { WidgetView } from '@/components/widgets/WidgetView';
import type { Page } from '@/types/model';
import { widgetKey } from '@/utils/widgets';

type ModelPageProps = { page: Page };

/** One page of the model: its title and its widget tree. */
export function ModelPage({ page }: ModelPageProps) {
  return (
    <>
      <h2>{page.title || page.name}</h2>
      {page.widgets.map((widget, index) => (
        <WidgetView key={widgetKey(widget, index)} widget={widget} context={null} />
      ))}
    </>
  );
}
