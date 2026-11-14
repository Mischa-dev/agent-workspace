import { lazy } from 'react'

// The workspace's own pages, loaded when first shown. The route table
// (`routes.tsx`) says where each one lives; each module's pages come from
// its registration (`<folder>/module.ts`).

export const SettingsArea = lazy(() => import('../settings/SettingsArea'))
export const SignInPage = lazy(() => import('../auth/SignInPage').then((module) => ({ default: module.SignInPage })))
export const ForgotPasswordPage = lazy(() => import('../auth/PasswordResetPages').then((module) => ({ default: module.ForgotPasswordPage })))
export const ResetPasswordPage = lazy(() => import('../auth/PasswordResetPages').then((module) => ({ default: module.ResetPasswordPage })))
export const VerifyEmailPage = lazy(() => import('../auth/PasswordResetPages').then((module) => ({ default: module.VerifyEmailPage })))
export const JoinPage = lazy(() => import('../invitations/JoinPage').then((module) => ({ default: module.JoinPage })))
export const HomePage = lazy(() => import('../projects/HomePage').then((module) => ({ default: module.HomePage })))
export const OnboardingPage = lazy(() => import('../onboarding/OnboardingPage').then((module) => ({ default: module.OnboardingPage })))
export const NotFoundPage = lazy(() => import('./NotFoundPage').then((module) => ({ default: module.NotFoundPage })))
export const DesignSystemPage = lazy(() => import('../design/DesignSystemPage').then((module) => ({ default: module.DesignSystemPage })))
export const PieceGallery = lazy(() => import('../pieces/PieceGallery').then((module) => ({ default: module.PieceGallery })))
export const SearchPage = lazy(() => import('../memory/SearchPage').then((module) => ({ default: module.SearchPage })))
