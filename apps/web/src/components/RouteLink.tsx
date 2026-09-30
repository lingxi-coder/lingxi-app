import { AnchorHTMLAttributes, MouseEvent } from 'react';
import { AppRoute, navigateTo, resolveRoute } from '../app/router';

interface RouteLinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> {
  href: AppRoute;
}

export function RouteLink({ children, className, href, onClick, ...anchorProps }: RouteLinkProps) {
  const handleClick = (event: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(event);
    if (
      event.defaultPrevented ||
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
    <a {...anchorProps} className={className} href={href} onClick={handleClick}>
      {children}
    </a>
  );
}
