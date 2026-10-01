import type { NavigationItem } from '@/types/model';

type NavigationProps = { items: NavigationItem[]; onOpen: (page: string) => void };

function NavigationList({ items, onOpen }: NavigationProps) {
  return (
    <ul>
      {items.map((item, index) => (
        <li key={`${item.page || item.microflow || item.caption}-${index}`}>
          <button type="button" onClick={() => item.page && onOpen(item.page)}>
            {item.caption || item.page || item.microflow}
          </button>
          {item.items?.length ? <NavigationList items={item.items} onOpen={onOpen} /> : null}
        </li>
      ))}
    </ul>
  );
}

/** The navigation profile's menu, nested the way the model nests it. */
export function Navigation({ items, onOpen }: NavigationProps) {
  return (
    <nav aria-label="Application">
      <NavigationList items={items} onOpen={onOpen} />
    </nav>
  );
}
