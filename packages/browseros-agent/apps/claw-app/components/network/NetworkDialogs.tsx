import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { networkCopy } from '@/screens/network/network.copy'

const copy = networkCopy

export function ClearRecordsDialog({
  open,
  count,
  onOpenChange,
  onConfirm,
}: {
  open: boolean
  count: number
  onOpenChange(open: boolean): void
  onConfirm(): void
}) {
  return (
    <AlertDialog onOpenChange={onOpenChange} open={open}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{copy.capture.clearTitle(count)}</AlertDialogTitle>
          <AlertDialogDescription>
            {copy.capture.clearBody}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{copy.capture.cancel}</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>
            {copy.capture.clearConfirm}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

export function DeleteAllRulesDialog({
  open,
  count,
  onOpenChange,
  onConfirm,
}: {
  open: boolean
  count: number
  onOpenChange(open: boolean): void
  onConfirm(): void
}) {
  return (
    <AlertDialog onOpenChange={onOpenChange} open={open}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {copy.rules.menu.deleteAllTitle(count)}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {copy.rules.menu.deleteAllBody}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{copy.capture.cancel}</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>
            {copy.rules.menu.deleteAllConfirm}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
