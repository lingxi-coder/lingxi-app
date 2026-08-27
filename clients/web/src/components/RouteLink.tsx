import { MouseEvent, PropsWithChildren } from 'react';
import { AppRoute, navigateTo, resolveRoute } from '../app/router';

interface RouteLinkProps extends PropsWithChildren {
  className?: string;
  href: AppRoute;
}

export function RouteLink({ children, className, href }: RouteLinkProps) {
  const handleClick = (event: MouseEvent<HTMLAnchorElement>) => {
    if (
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey ||
      event.currentTarget.target === '_blank'
    ) {
      return;
    }
    event.preventDefault();
    navigateTo(resolveRoute(href));
  };

  return (
    <a className={className} href={href} onClick={handleClick}>
      {children}
    </a>
  );
}
