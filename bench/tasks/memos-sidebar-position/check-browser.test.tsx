import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it } from 'vitest';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';

it('keeps a fixed sidebar popup in place when the page scrolls', async () => {
  document.body.style.height = '3000px';
  render(
    <div style={{ position: 'fixed', top: '100px', left: '40px' }}>
      <Popover>
        <PopoverTrigger>Open sidebar menu</PopoverTrigger>
        <PopoverContent positionMethod="fixed">Sidebar popup</PopoverContent>
      </Popover>
    </div>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Open sidebar menu' }));
  const popup = await screen.findByText('Sidebar popup');
  const positioner = popup.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  const before = positioner.getBoundingClientRect().top;
  window.scrollTo(0, 300);
  expect(window.scrollY).toBeGreaterThan(0);
  expect(Math.abs(positioner.getBoundingClientRect().top - before)).toBeLessThan(2);
});

it('preserves the default positioning for other popovers', async () => {
  render(
    <Popover>
      <PopoverTrigger>Open normal popup</PopoverTrigger>
      <PopoverContent>Normal popup</PopoverContent>
    </Popover>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Open normal popup' }));
  const popup = await screen.findByText('Normal popup');
  const positioner = popup.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  expect(positioner.style.position).toBe('absolute');
});

it('preserves the default positioning for other dropdown menus', async () => {
  render(
    <DropdownMenu>
      <DropdownMenuTrigger>Open normal menu</DropdownMenuTrigger>
      <DropdownMenuContent><DropdownMenuItem>Menu item</DropdownMenuItem></DropdownMenuContent>
    </DropdownMenu>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Open normal menu' }));
  const menu = await screen.findByRole('menu');
  const positioner = menu.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  expect(positioner.style.position).toBe('absolute');
});

it('preserves the default positioning for other selects', async () => {
  render(
    <Select defaultValue="one" items={[{ value: 'one', label: 'One' }]}>
      <SelectTrigger aria-label="Select one"><SelectValue /></SelectTrigger>
      <SelectContent><SelectItem value="one">One</SelectItem></SelectContent>
    </Select>,
  );
  fireEvent.click(screen.getByRole('combobox', { name: 'Select one' }));
  const option = await screen.findByRole('option', { name: 'One' });
  const positioner = option.closest('[data-slot="select-content"]')?.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  expect(positioner.style.position).toBe('absolute');
});
