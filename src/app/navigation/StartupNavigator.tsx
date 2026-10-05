import React, {useCallback, useEffect, useState} from 'react';
import {BackHandler} from 'react-native';
import {
  HowVoidWorksScreen,
  InitializingScreen,
  SecurityNoticeScreen,
  TermsScreen,
  WelcomeScreen,
} from '../../startup';

type StartupStep =
  | 'welcome'
  | 'how-it-works'
  | 'security'
  | 'terms'
  | 'initializing';

interface StartupNavigatorProps {
  onPreview: () => void;
}

export function StartupNavigator({onPreview}: StartupNavigatorProps) {
  const [step, setStep] = useState<StartupStep>('welcome');
  const [securityAcknowledged, setSecurityAcknowledged] = useState(false);
  const [termsAccepted, setTermsAccepted] = useState(false);

  const goBack = useCallback(() => {
    switch (step) {
      case 'how-it-works':
        setStep('welcome');
        break;
      case 'security':
        setStep('how-it-works');
        break;
      case 'terms':
        setStep('security');
        break;
      case 'initializing':
        setStep('terms');
        break;
    }
  }, [step]);

  useEffect(() => {
    const subscription = BackHandler.addEventListener('hardwareBackPress', () => {
      if (step === 'welcome') {
        return false;
      }
      goBack();
      return true;
    });
    return () => subscription.remove();
  }, [goBack, step]);

  switch (step) {
    case 'welcome':
      return <WelcomeScreen onContinue={() => setStep('how-it-works')} />;
    case 'how-it-works':
      return (
        <HowVoidWorksScreen
          onBack={goBack}
          onContinue={() => setStep('security')}
        />
      );
    case 'security':
      return (
        <SecurityNoticeScreen
          onBack={goBack}
          acknowledged={securityAcknowledged}
          onAcknowledgedChange={setSecurityAcknowledged}
          onContinue={() => {
            if (securityAcknowledged) {
              setStep('terms');
            }
          }}
        />
      );
    case 'terms':
      return (
        <TermsScreen
          onBack={goBack}
          accepted={termsAccepted}
          onAcceptedChange={setTermsAccepted}
          onInitialize={() => {
            if (securityAcknowledged && termsAccepted) {
              setStep('initializing');
            }
          }}
        />
      );
    case 'initializing':
      return <InitializingScreen onBack={goBack} onPreview={onPreview} />;
  }
}
