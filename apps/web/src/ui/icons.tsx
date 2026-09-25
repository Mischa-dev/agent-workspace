import {
  Archive,
  ArrowDown,
  ArrowLeft,
  AtSign,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CircleMinus,
  CircleQuestionMark,
  CircleStop,
  Clock,
  Copy,
  ExternalLink,
  GitBranch,
  GitPullRequest,
  Hash,
  House,
  Info,
  Link,
  LoaderCircle,
  Lock,
  LogOut,
  Menu,
  MessageSquare,
  MessagesSquare,
  Palette,
  PanelRight,
  Plus,
  RotateCcw,
  Search,
  SendHorizontal,
  Settings,
  ShieldCheck,
  SquareCheckBig,
  SquarePen,
  Star,
  Terminal,
  Trash,
  TriangleAlert,
  Undo2,
  Users,
  WifiOff,
  X,
  type LucideIcon,
} from 'lucide-react'

const icons = {
  archive: Archive,
  'arrow-down': ArrowDown,
  'arrow-left': ArrowLeft,
  'at-sign': AtSign,
  bot: Bot,
  branch: GitBranch,
  check: Check,
  'chevron-down': ChevronDown,
  'chevron-right': ChevronRight,
  'circle-alert': CircleAlert,
  'circle-check': CircleCheck,
  'circle-help': CircleQuestionMark,
  'circle-minus': CircleMinus,
  clock: Clock,
  compose: SquarePen,
  copy: Copy,
  external: ExternalLink,
  hash: Hash,
  home: House,
  info: Info,
  link: Link,
  loader: LoaderCircle,
  lock: Lock,
  'log-out': LogOut,
  menu: Menu,
  message: MessageSquare,
  messages: MessagesSquare,
  palette: Palette,
  'panel-right': PanelRight,
  plus: Plus,
  'pull-request': GitPullRequest,
  retry: RotateCcw,
  search: Search,
  send: SendHorizontal,
  settings: Settings,
  'shield-check': ShieldCheck,
  star: Star,
  stop: CircleStop,
  terminal: Terminal,
  trash: Trash,
  'triangle-alert': TriangleAlert,
  undo: Undo2,
  users: Users,
  'wifi-off': WifiOff,
  work: SquareCheckBig,
  x: X,
} satisfies Record<string, LucideIcon>

export type IconName = keyof typeof icons

/** A Lucide icon. Decorative unless given a label. */
export function Icon({
  className,
  label,
  name,
  size = 16,
}: {
  className?: string
  label?: string
  name: IconName
  size?: number
}) {
  const Glyph = icons[name]
  return (
    <Glyph
      absoluteStrokeWidth={false}
      aria-hidden={label ? undefined : true}
      aria-label={label}
      className={className ? `aw-icon ${className}` : 'aw-icon'}
      focusable="false"
      role={label ? 'img' : undefined}
      size={size}
      strokeWidth={size <= 14 ? 2.25 : size <= 18 ? 2 : 1.75}
    />
  )
}
